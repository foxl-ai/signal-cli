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
use presage::proto::sync_message::Sent;
use presage::proto::{attachment_pointer, AttachmentPointer, EditMessage, SyncMessage};
use presage::store::{ContentExt, Store};
use presage::Manager;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use tempfile::{Builder, TempDir};
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};
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
        if let Received::QueueEmpty = content {
            break;
        }
    }
    Ok(())
}

async fn print_content<S: Store>(
    _manager: &Manager<S, Registered>,
    content: &Content,
    format: &OutputFormat,
    attachments: Vec<JsonAttachment>,
) {
    let Ok(thread) = presage::store::Thread::try_from(content) else {
        return;
    };

    let sender = content.metadata.sender.service_id_string();
    let ts = content.timestamp();

    let (msg_type, body) = match &content.body {
        ContentBody::DataMessage(DataMessage { body, .. }) => ("message", body.clone()),
        ContentBody::SynchronizeMessage(SyncMessage {
            sent:
                Some(Sent {
                    message: Some(DataMessage { body, .. }),
                    ..
                }),
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
        attachments,
    };

    print_result(format, &jm);
}

/// Every attachment gets its own name, also across messages that share a timestamp.
static ATTACHMENT_SEQ: AtomicU64 = AtomicU64::new(0);

/// The attachments a received message carries: one sent to this account, or one the account
/// sent from another of its devices (a sync, e.g. Note to Self).
fn attachments_of(content: &Content) -> &[AttachmentPointer] {
    match &content.body {
        ContentBody::DataMessage(DataMessage { attachments, .. }) => attachments,
        ContentBody::SynchronizeMessage(SyncMessage {
            sent:
                Some(Sent {
                    message: Some(DataMessage { attachments, .. }),
                    ..
                }),
            ..
        }) => attachments,
        _ => &[],
    }
}

/// `AttachmentPointer.Flags.VOICE_MESSAGE`: the sender's app recorded it as a voice message.
fn is_voice_note(ap: &AttachmentPointer) -> bool {
    ap.flags.unwrap_or(0) & (attachment_pointer::Flags::VoiceMessage as u32) != 0
}

/// The extension for a stated type: mime_guess's first one when it is short ASCII
/// alphanumerics, else `bin`. The type is the sender's, so nothing else of it reaches a path.
fn extension_for(content_type: Option<&str>) -> String {
    let stated = content_type.unwrap_or("").trim();
    // No type, or the generic one (whose first extension in mime_guess is "aaf"): plain bytes.
    if stated.is_empty() || stated.eq_ignore_ascii_case(APPLICATION_OCTET_STREAM.as_ref()) {
        return "bin".to_string();
    }
    mime_guess::get_mime_extensions_str(stated)
        .and_then(|e| e.first().copied())
        .filter(|e| !e.is_empty() && e.len() <= 8 && e.chars().all(|c| c.is_ascii_alphanumeric()))
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| "bin".to_string())
}

/// The name an attachment is saved under, chosen here and never taken from the sender: the
/// sender's `file_name` is reported in the JSON as data only.
fn attachment_file_name(timestamp: u64, seq: u64, extension: &str) -> String {
    format!("{timestamp}-{seq}.{extension}")
}

/// Where an attachment is saved: directly inside `dir`, whatever the sender wrote.
fn attachment_path(
    dir: &std::path::Path,
    timestamp: u64,
    seq: u64,
    ap: &AttachmentPointer,
) -> std::path::PathBuf {
    dir.join(attachment_file_name(
        timestamp,
        seq,
        &extension_for(ap.content_type.as_deref()),
    ))
}

/// Download and save every attachment of `content` into `dir`, each under a fresh name, never
/// writing through anything already at that path. Returns what was saved, for the message's
/// JSON; a download or write that fails is logged and left out.
async fn save_attachments<S: Store>(
    manager: &Manager<S, Registered>,
    content: &Content,
    dir: &std::path::Path,
) -> Vec<JsonAttachment> {
    let mut saved = Vec::new();
    for ap in attachments_of(content) {
        let data = match manager.get_attachment(ap).await {
            Ok(data) => data,
            Err(e) => {
                error!("failed to download attachment: {e}");
                continue;
            }
        };
        let seq = ATTACHMENT_SEQ.fetch_add(1, Ordering::Relaxed);
        let path = attachment_path(dir, content.timestamp(), seq, ap);
        let written = async {
            let mut file = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .await?;
            file.write_all(&data).await?;
            file.flush().await
        }
        .await;
        match written {
            Ok(()) => {
                info!(path = %path.display(), "saved attachment");
                saved.push(JsonAttachment {
                    path: path.display().to_string(),
                    content_type: ap.content_type.clone(),
                    file_name: ap.file_name.clone(),
                    size: data.len() as u64,
                    voice_note: is_voice_note(ap),
                });
            }
            Err(e) => error!("failed to save attachment: {e}"),
        }
    }
    saved
}

#[cfg(test)]
mod attachment_name_tests {
    use super::*;

    #[test]
    fn the_saved_name_is_generated_never_the_senders() {
        let name = attachment_file_name(1758844800123, 7, "ogg");
        assert_eq!(name, "1758844800123-7.ogg");
        let dir = std::path::Path::new("/tmp/signal-cli-attachments-x");
        // Whatever the sender called the file, the path stays a direct child of the directory.
        assert_eq!(dir.join(&name).parent(), Some(dir));
        assert!(!name.contains('/') && !name.contains('\\') && !name.contains(".."));
    }

    #[test]
    fn a_senders_file_name_never_leaves_the_directory() {
        let dir = std::path::Path::new("/tmp/signal-cli-attachments-x");
        for name in [
            "/Users/someone/Library/LaunchAgents/x.plist",
            "../../../.zshrc",
            "..",
            "a/b/c.jpg",
            "voice-message.ogg",
        ] {
            let ap = AttachmentPointer {
                file_name: Some(name.to_string()),
                content_type: Some("audio/ogg".to_string()),
                ..Default::default()
            };
            let path = attachment_path(dir, 1758844800123, 0, &ap);
            assert_eq!(path.parent(), Some(dir), "{name} -> {}", path.display());
            let expected = format!("1758844800123-0.{}", extension_for(Some("audio/ogg")));
            assert_eq!(
                path.file_name().and_then(|n| n.to_str()),
                Some(expected.as_str()),
                "{name}"
            );
        }
    }

    #[test]
    fn the_extension_is_short_ascii_or_bin() {
        // mime_guess's own first extension for the type (audio/ogg is "oga" there).
        assert!(["oga", "ogg"].contains(&extension_for(Some("audio/ogg")).as_str()));
        assert_eq!(extension_for(Some("audio/aac")), "aac");
        assert_eq!(extension_for(Some("image/jpeg")).len() <= 8, true);
        assert_eq!(extension_for(None), "bin");
        assert_eq!(extension_for(Some("../../etc/passwd")), "bin");
        assert_eq!(extension_for(Some("x/unknown-type")), "bin");
    }
}

pub async fn run<S: Store + Send>(
    command: Command,
    store: S,
    format: OutputFormat,
) -> anyhow::Result<()> {
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
            let mut retry_count: u32 = 0;
            let max_retries = if stop_after_empty_queue { 0 } else { u32::MAX };

            loop {
                info!("connecting to message stream...");
                let messages = match manager.receive_messages().await {
                    Ok(m) => m,
                    Err(e) => {
                        if retry_count >= max_retries {
                            return Err(e.into());
                        }
                        retry_count += 1;
                        let delay = std::cmp::min(retry_count * 2, 30);
                        tracing::warn!(
                            "failed to connect (attempt {retry_count}), retrying in {delay}s: {e}"
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(delay as u64)).await;
                        continue;
                    }
                };
                pin_mut!(messages);
                retry_count = 0; // reset on successful connection

                while let Some(content) = messages.next().await {
                    match content {
                        Received::QueueEmpty => {
                            println!("-- queue empty --");
                            if stop_after_empty_queue {
                                return Ok(());
                            }
                        }
                        Received::Contacts => println!("-- contacts synced --"),
                        Received::Content(content) => {
                            // Saved FIRST, so every path printed with the message exists when a
                            // reader acts on it.
                            let attachments =
                                save_attachments(&manager, &content, tmp_dir.path()).await;
                            print_content(&manager, &content, &format, attachments).await;
                        }
                    }
                }

                // Stream ended (websocket closed)
                if stop_after_empty_queue {
                    break;
                }
                tracing::warn!("message stream ended, reconnecting in 5s...");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
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
                print_content(&manager, &msg, &format, Vec::new()).await;
            }
        }

        Command::Whoami => {
            let manager = load_and_receive(store).await?;
            let whoami = manager.whoami().await?;
            match format {
                OutputFormat::Json => println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "uuid": whoami.aci.to_string(),
                    }))?
                ),
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
            println!(
                "  Next pre-key ID: {}",
                aci.next_pre_key_id().await.unwrap_or(0)
            );
            println!(
                "  Next signed pre-key ID: {}",
                aci.next_signed_pre_key_id().await.unwrap_or(0)
            );
            println!(
                "  Next kyber pre-key ID: {}",
                aci.next_pq_pre_key_id().await.unwrap_or(0)
            );
            println!(
                "  Signed pre-keys: {}",
                aci.signed_pre_keys_count().await.unwrap_or(0)
            );
            println!(
                "  Kyber pre-keys: {}",
                aci.kyber_pre_keys_count(false).await.unwrap_or(0)
            );
            println!(
                "  Kyber last-resort: {}",
                aci.kyber_pre_keys_count(true).await.unwrap_or(0)
            );
            println!("\nPNI Pre-key stats:");
            println!(
                "  Next pre-key ID: {}",
                pni.next_pre_key_id().await.unwrap_or(0)
            );
            println!(
                "  Next signed pre-key ID: {}",
                pni.next_signed_pre_key_id().await.unwrap_or(0)
            );
            println!(
                "  Next kyber pre-key ID: {}",
                pni.next_pq_pre_key_id().await.unwrap_or(0)
            );
            println!(
                "  Signed pre-keys: {}",
                pni.signed_pre_keys_count().await.unwrap_or(0)
            );
            println!(
                "  Kyber pre-keys: {}",
                pni.kyber_pre_keys_count(false).await.unwrap_or(0)
            );
            println!(
                "  Kyber last-resort: {}",
                pni.kyber_pre_keys_count(true).await.unwrap_or(0)
            );
        }
    }

    Ok(())
}
