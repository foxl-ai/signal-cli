use std::time::UNIX_EPOCH;

use anyhow::Context as _;
use futures::{pin_mut, StreamExt};
use mime_guess::mime::APPLICATION_OCTET_STREAM;
use presage::libsignal_service::content::{Content, ContentBody, DataMessage, GroupContextV2};
use presage::libsignal_service::pre_keys::PreKeysStore;
use presage::libsignal_service::protocol::ServiceId;
use presage::libsignal_service::sender::AttachmentSpec;
use presage::libsignal_service::zkgroup::GroupMasterKeyBytes;
use presage::manager::{Registered, RegistrationOptions};
use presage::model::messages::Received;
use presage::proto::{EditMessage, SyncMessage};
use presage::store::{ContentExt, Store};
use presage::Manager;
use presage::proto::sync_message::Sent;
use std::path::PathBuf;
use tempfile::{Builder, TempDir};
use tokio::io::{self, AsyncBufReadExt, BufReader};
use tracing::{debug, error, info};

use crate::cli::Command;
use crate::output::*;

enum Recipient {
    Contact(uuid::Uuid),
    Group(GroupMasterKeyBytes),
}

async fn send_message<S: Store>(
    manager: &mut Manager<S, Registered>,
    recipient: Recipient,
    msg: impl Into<ContentBody>,
) -> anyhow::Result<()> {
    let timestamp = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_millis() as u64;

    let mut content_body = msg.into();
    if let ContentBody::DataMessage(d) = &mut content_body {
        d.timestamp = Some(timestamp);
    }

    match recipient {
        Recipient::Contact(uuid) => {
            info!(recipient = %uuid, "sending message");
            manager
                .send_message(ServiceId::Aci(uuid.into()), content_body, timestamp)
                .await
                .expect("failed to send message");
        }
        Recipient::Group(master_key) => {
            info!("sending message to group");
            manager
                .send_message_to_group(&master_key, content_body, timestamp)
                .await
                .expect("failed to send group message");
        }
    }

    Ok(())
}

async fn upload_attachments<S: Store>(
    paths: Vec<PathBuf>,
    manager: &Manager<S, Registered>,
) -> anyhow::Result<Vec<presage::proto::AttachmentPointer>> {
    let specs: Vec<_> = paths
        .into_iter()
        .filter_map(|path| {
            let data = std::fs::read(&path).ok()?;
            Some((
                AttachmentSpec {
                    content_type: mime_guess::from_path(&path)
                        .first()
                        .unwrap_or(APPLICATION_OCTET_STREAM)
                        .to_string(),
                    length: data.len(),
                    file_name: path.file_name().map(|s| s.to_string_lossy().to_string()),
                    preview: None,
                    voice_note: None,
                    borderless: None,
                    width: None,
                    height: None,
                    caption: None,
                    blur_hash: None,
                },
                data,
            ))
        })
        .collect();

    let results: Result<Vec<_>, _> = manager
        .upload_attachments(specs)
        .await?
        .into_iter()
        .collect();
    Ok(results?)
}

fn attachments_tmp_dir() -> anyhow::Result<TempDir> {
    let dir = Builder::new().prefix("signal-cli-attachments").tempdir()?;
    info!(path = %dir.path().display(), "attachments directory");
    Ok(dir)
}

async fn load_registered<S: Store + Send>(store: S) -> anyhow::Result<Manager<S, Registered>> {
    Manager::load_registered(store).await.map_err(Into::into)
}

async fn load_and_receive<S: Store + Send>(store: S) -> anyhow::Result<Manager<S, Registered>> {
    let manager = Manager::load_registered(store).await?;
    let bg = manager.clone();
    tokio::task::spawn_local(async move {
        if let Err(e) = receive_silent(bg).await {
            debug!("background receive ended: {e}");
        }
    });
    Ok(manager)
}

async fn receive_silent<S: Store>(mut manager: Manager<S, Registered>) -> anyhow::Result<()> {
    let messages = manager.receive_messages().await?;
    pin_mut!(messages);
    while let Some(content) = messages.next().await {
        match content {
            Received::QueueEmpty => break,
            _ => {}
        }
    }
    Ok(())
}

async fn print_content<S: Store>(
    _manager: &Manager<S, Registered>,
    content: &Content,
    format: &OutputFormat,
) {
    let Ok(thread) = presage::store::Thread::try_from(content) else {
        return;
    };

    let sender = content.metadata.sender.service_id_string();
    let ts = content.timestamp();

    let (msg_type, body) = match &content.body {
        ContentBody::DataMessage(DataMessage { body, .. }) => {
            ("message", body.clone())
        }
        ContentBody::SynchronizeMessage(SyncMessage {
            sent: Some(Sent { message: Some(DataMessage { body, .. }), .. }),
            ..
        }) => ("sync", body.clone()),
        ContentBody::TypingMessage(_) => ("typing", None),
        ContentBody::ReceiptMessage(_) => ("receipt", None),
        ContentBody::CallMessage(_) => ("call", None),
        ContentBody::EditMessage(EditMessage {
            data_message: Some(DataMessage { body, .. }),
            ..
        }) => ("edit", body.clone()),
        _ => ("other", None),
    };

    let thread_str = format!("{thread}");
    let jm = JsonMessage {
        timestamp: ts,
        sender: sender.clone(),
        thread: thread_str,
        body,
        r#type: msg_type.to_string(),
    };

    print_result(format, &jm);
}

pub async fn run<S: Store + Send>(command: Command, store: S, format: OutputFormat) -> anyhow::Result<()> {
    match command {
        Command::Register {
            servers,
            phone_number,
            use_voice_call,
            captcha,
            force,
        } => {
            let manager = Manager::register(
                store,
                RegistrationOptions {
                    signal_servers: servers,
                    phone_number,
                    use_voice_call,
                    captcha: Some(captcha.host_str().unwrap()),
                    force,
                },
            )
            .await?;

            println!("Enter verification code:");
            let stdin = io::stdin();
            let reader = BufReader::new(stdin);
            if let Some(code) = reader.lines().next_line().await? {
                let registered = manager.confirm_verification_code(code).await?;
                println!("Registered: {}", registered.registration_data().service_ids);
            }
        }

        Command::LinkDevice {
            servers,
            device_name,
        } => {
            let (tx, rx) = futures::channel::oneshot::channel();
            let (result, _) = futures::future::join(
                Manager::link_secondary_device(store, servers, device_name, tx),
                async {
                    match rx.await {
                        Ok(url) => {
                            println!("Scan this QR code on your primary device:");
                            qr2term::print_qr(url.to_string()).expect("failed to render QR");
                            println!("Or use URL: {url}");
                        }
                        Err(e) => error!("linking cancelled: {e}"),
                    }
                },
            )
            .await;

            match result {
                Ok(manager) => {
                    let whoami = manager.whoami().await?;
                    println!("Linked! {whoami:?}");
                }
                Err(e) => anyhow::bail!("linking failed: {e}"),
            }
        }

        Command::Send {
            uuid,
            phone_number,
            message,
            attachment,
        } => {
            let mut manager = load_and_receive(store).await?;

            let target_uuid = if let Some(uuid) = uuid {
                uuid
            } else if let Some(_phone) = phone_number {
                #[cfg(feature = "cdsi")]
                {
                    let resolved = manager
                        .discover_contacts_by_phone_number(vec![_phone.clone()])
                        .await?;
                    let (_, identity) = resolved
                        .into_iter()
                        .next()
                        .context("no results from CDSI")?;
                    identity
                        .context("phone number not registered on Signal")?
                        .raw_uuid()
                }
                #[cfg(not(feature = "cdsi"))]
                {
                    anyhow::bail!("CDSI feature not enabled, use --uuid instead");
                }
            } else {
                anyhow::bail!("provide either --uuid or --phone-number");
            };

            let attachments = upload_attachments(attachment, &manager).await?;
            let data_message = DataMessage {
                body: Some(message),
                attachments,
                ..Default::default()
            };

            send_message(&mut manager, Recipient::Contact(target_uuid), data_message).await?;
            println!("Message sent!");
        }

        Command::SendToGroup {
            master_key,
            message,
            attachment,
        } => {
            let mut manager = load_and_receive(store).await?;
            let attachments = upload_attachments(attachment, &manager).await?;
            let data_message = DataMessage {
                body: Some(message),
                attachments,
                group_v2: Some(GroupContextV2 {
                    master_key: Some(master_key.to_vec()),
                    revision: Some(0),
                    ..Default::default()
                }),
                ..Default::default()
            };
            send_message(&mut manager, Recipient::Group(master_key), data_message).await?;
            println!("Group message sent!");
        }

        Command::Receive {
            notifications: _,
            stop_after_empty_queue,
        } => {
            let mut manager = load_registered(store).await?;
            let tmp_dir = attachments_tmp_dir()?;
            let messages = manager
                .receive_messages()
                .await
                .context("failed to start receiving")?;
            pin_mut!(messages);

            while let Some(content) = messages.next().await {
                match content {
                    Received::QueueEmpty => {
                        println!("-- queue empty --");
                        if stop_after_empty_queue {
                            break;
                        }
                    }
                    Received::Contacts => println!("-- contacts synced --"),
                    Received::Content(content) => {
                        print_content(&manager, &content, &format).await;

                        // Save attachments
                        if let ContentBody::DataMessage(DataMessage { attachments, .. }) =
                            &content.body
                        {
                            for ap in attachments {
                                if let Ok(data) = manager.get_attachment(ap).await {
                                    let ext = mime_guess::get_mime_extensions_str(
                                        ap.content_type.as_deref().unwrap_or("application/octet-stream"),
                                    )
                                    .and_then(|e| e.first())
                                    .unwrap_or(&"bin");
                                    let fname = ap
                                        .file_name
                                        .clone()
                                        .unwrap_or_else(|| format!("attachment.{ext}"));
                                    let path = tmp_dir.path().join(&fname);
                                    if let Err(e) = tokio::fs::write(&path, &data).await {
                                        error!("failed to save attachment: {e}");
                                    } else {
                                        info!(path = %path.display(), "saved attachment");
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Command::ListGroups { name_filter } => {
            let manager = load_and_receive(store).await?;
            let groups = manager.store().groups().await?;
            for group in groups {
                match group {
                    Ok((key, g)) => {
                        if let Some(ref filter) = name_filter {
                            if !g.title.contains(filter) {
                                continue;
                            }
                        }
                        let jg = JsonGroup {
                            master_key: hex::encode(key),
                            title: g.title,
                            description: g.description,
                            members_count: g.members.len(),
                            revision: g.revision,
                        };
                        print_result(&format, &jg);
                    }
                    Err(e) => error!("failed to read group: {e}"),
                }
            }
        }

        Command::ListContacts => {
            let manager = load_and_receive(store).await?;
            for contact in manager.store().contacts().await?.flatten() {
                let jc = JsonContact {
                    uuid: contact.uuid.to_string(),
                    phone_number: contact.phone_number.as_ref().map(|p| p.to_string()),
                    name: contact.name.clone(),
                };
                print_result(&format, &jc);
            }
        }

        Command::ListMessages {
            recipient,
            group,
            from,
        } => {
            let manager = load_and_receive(store).await?;
            let thread = match (group, recipient) {
                (Some(key), _) => presage::store::Thread::Group(key),
                (_, Some(id)) => presage::store::Thread::Contact(id),
                _ => anyhow::bail!("provide --recipient or --group"),
            };
            for msg in manager
                .store()
                .messages(&thread, from.unwrap_or(0)..)
                .await?
                .filter_map(Result::ok)
            {
                print_content(&manager, &msg, &format).await;
            }
        }

        Command::Whoami => {
            let manager = load_and_receive(store).await?;
            let whoami = manager.whoami().await?;
            match format {
                OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                    "uuid": whoami.aci.to_string(),
                }))?),
                OutputFormat::Text => println!("{whoami:?}"),
            }
        }

        Command::ListDevices => {
            let manager = load_and_receive(store).await?;
            let devices = manager.devices().await?;
            let current = manager.device_id();
            for d in devices {
                let jd = JsonDevice {
                    id: u32::from(d.id),
                    name: d.name.clone(),
                    created: d.created_at.to_string(),
                    last_seen: d.last_seen.to_string(),
                    is_current: d.id == current,
                };
                print_result(&format, &jd);
            }
        }

        #[cfg(feature = "cdsi")]
        Command::Resolve { phone_number } => {
            let mut manager = load_and_receive(store).await?;
            let resolved = manager
                .discover_contacts_by_phone_number(phone_number)
                .await?;
            for (number, identity) in resolved {
                match identity {
                    Some(id) => println!("{number} => {}", id.service_id_string()),
                    None => println!("{number} => not registered"),
                }
            }
        }

        Command::SyncContacts => {
            let mut manager = load_and_receive(store).await?;
            manager.request_contacts().await?;
            let messages = manager
                .receive_messages()
                .await
                .context("failed to start receiving")?;
            pin_mut!(messages);
            println!("Syncing contacts...");
            while let Some(content) = messages.next().await {
                match content {
                    Received::QueueEmpty => break,
                    Received::Contacts => {
                        println!("Contacts synced!");
                        break;
                    }
                    Received::Content(_) => print!("."),
                }
            }
        }

        Command::GetProfile { uuid } => {
            let mut manager = load_and_receive(store).await?;
            // Try to find profile key from contacts
            let mut profile_key = None;
            for contact in manager
                .store()
                .contacts()
                .await?
                .filter_map(Result::ok)
                .filter(|c| c.uuid == uuid)
            {
                if let Ok(pk) = contact.profile_key.try_into() {
                    let pk: [u8; 32] = pk;
                    profile_key = Some(presage::libsignal_service::prelude::ProfileKey::create(pk));
                }
            }

            let profile = match profile_key {
                Some(pk) => manager.retrieve_profile_by_uuid(uuid, pk).await?,
                None => manager.retrieve_profile().await?,
            };
            println!("{profile:#?}");
        }

        Command::Stats => {
            let manager = load_and_receive(store).await?;
            let aci = manager.store().aci_protocol_store();
            let pni = manager.store().pni_protocol_store();

            println!("ACI Pre-key stats:");
            println!("  Next pre-key ID: {}", aci.next_pre_key_id().await.unwrap_or(0));
            println!("  Next signed pre-key ID: {}", aci.next_signed_pre_key_id().await.unwrap_or(0));
            println!("  Next kyber pre-key ID: {}", aci.next_pq_pre_key_id().await.unwrap_or(0));
            println!("  Signed pre-keys: {}", aci.signed_pre_keys_count().await.unwrap_or(0));
            println!("  Kyber pre-keys: {}", aci.kyber_pre_keys_count(false).await.unwrap_or(0));
            println!("  Kyber last-resort: {}", aci.kyber_pre_keys_count(true).await.unwrap_or(0));
            println!("\nPNI Pre-key stats:");
            println!("  Next pre-key ID: {}", pni.next_pre_key_id().await.unwrap_or(0));
            println!("  Next signed pre-key ID: {}", pni.next_signed_pre_key_id().await.unwrap_or(0));
            println!("  Next kyber pre-key ID: {}", pni.next_pq_pre_key_id().await.unwrap_or(0));
            println!("  Signed pre-keys: {}", pni.signed_pre_keys_count().await.unwrap_or(0));
            println!("  Kyber pre-keys: {}", pni.kyber_pre_keys_count(false).await.unwrap_or(0));
            println!("  Kyber last-resort: {}", pni.kyber_pre_keys_count(true).await.unwrap_or(0));
        }
    }

    Ok(())
}
