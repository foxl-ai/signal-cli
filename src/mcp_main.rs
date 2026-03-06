#[allow(dead_code)]
mod output;

use anyhow::Context as _;
use directories::ProjectDirs;
use presage::libsignal_service::configuration::SignalServers;
use presage::libsignal_service::content::{ContentBody, DataMessage};
use presage::libsignal_service::protocol::ServiceId;
use presage::manager::Registered;
use presage::model::identity::OnNewIdentity;
use presage::store::ContentsStore;
use presage::Manager;
use presage_store_sqlite::SqliteStore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::time::UNIX_EPOCH;
use tokio::runtime::Runtime;

fn default_db_path() -> String {
    ProjectDirs::from("org", "whisperfish", "signal-cli-rust")
        .expect("could not determine config directory")
        .config_dir()
        .join("signal.db3")
        .display()
        .to_string()
}

fn ensure_parent_dir_exists(path: &str) -> anyhow::Result<()> {
    let parent = std::path::Path::new(path)
        .parent()
        .context("database path has no parent directory")?;
    std::fs::create_dir_all(parent)?;
    Ok(())
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    params: Option<Value>,
}

#[derive(Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Serialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

impl JsonRpcResponse {
    fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    fn error(id: Value, code: i64, message: String) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcError { code, message }),
        }
    }
}

struct McpServer {
    rt: Runtime,
    db_path: String,
}

impl McpServer {
    fn new() -> anyhow::Result<Self> {
        let db_path = std::env::var("SIGNAL_CLI_DB").unwrap_or_else(|_| default_db_path());
        ensure_parent_dir_exists(&db_path)?;

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;

        Ok(Self { rt, db_path })
    }

    async fn open_store(&self) -> anyhow::Result<SqliteStore> {
        let passphrase = std::env::var("SIGNAL_CLI_PASSPHRASE").ok();
        SqliteStore::open_with_passphrase(
            &self.db_path,
            passphrase.as_deref(),
            OnNewIdentity::Trust,
        )
        .await
        .map_err(Into::into)
    }

    fn handle_request(&self, req: JsonRpcRequest) -> JsonRpcResponse {
        let id = req.id.clone().unwrap_or(Value::Null);

        match req.method.as_str() {
            "initialize" => self.handle_initialize(id, req.params),
            "tools/list" => self.handle_tools_list(id),
            "tools/call" => self.handle_tools_call(id, req.params),
            "notifications/initialized" => JsonRpcResponse::success(id, json!({})),
            _ => JsonRpcResponse::error(id, -32601, format!("method not found: {}", req.method)),
        }
    }

    fn handle_initialize(&self, id: Value, _params: Option<Value>) -> JsonRpcResponse {
        JsonRpcResponse::success(
            id,
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "signal-cli-mcp",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        )
    }

    fn handle_tools_list(&self, id: Value) -> JsonRpcResponse {
        JsonRpcResponse::success(
            id,
            json!({
                "tools": [
                    {
                        "name": "send_message",
                        "description": "Send a Signal message to a contact by UUID",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "uuid": { "type": "string", "description": "Recipient UUID" },
                                "message": { "type": "string", "description": "Message text" }
                            },
                            "required": ["uuid", "message"]
                        }
                    },
                    {
                        "name": "send_message_by_phone",
                        "description": "Send a Signal message to a contact by phone number (requires CDSI)",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "phone_number": { "type": "string", "description": "Phone number in E.164 format" },
                                "message": { "type": "string", "description": "Message text" }
                            },
                            "required": ["phone_number", "message"]
                        }
                    },
                    {
                        "name": "list_contacts",
                        "description": "List all Signal contacts",
                        "inputSchema": { "type": "object", "properties": {} }
                    },
                    {
                        "name": "list_groups",
                        "description": "List all Signal groups",
                        "inputSchema": { "type": "object", "properties": {} }
                    },
                    {
                        "name": "receive_messages",
                        "description": "Receive pending messages",
                        "inputSchema": { "type": "object", "properties": {} }
                    },
                    {
                        "name": "whoami",
                        "description": "Get current account information",
                        "inputSchema": { "type": "object", "properties": {} }
                    },
                    {
                        "name": "resolve_phone",
                        "description": "Resolve a phone number to a Signal UUID",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "phone_number": { "type": "string", "description": "Phone number in E.164 format" }
                            },
                            "required": ["phone_number"]
                        }
                    },
                    {
                        "name": "list_messages",
                        "description": "List stored messages from local database for a specific contact or group thread",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "contact_uuid": { "type": "string", "description": "Contact UUID to list messages for (mutually exclusive with group_master_key)" },
                                "group_master_key": { "type": "string", "description": "Group master key (hex) to list messages for (mutually exclusive with contact_uuid)" },
                                "from_timestamp": { "type": "integer", "description": "Only return messages after this UNIX timestamp (milliseconds)" },
                                "limit": { "type": "integer", "description": "Maximum number of messages to return (default: 50)" }
                            }
                        }
                    },
                    {
                        "name": "link_device",
                        "description": "Link this CLI as a secondary device to an existing Signal account. Returns a provisioning URL and QR code text that the user must scan with their Signal mobile app (Settings > Linked Devices > Link New Device). The tool will wait for the linking to complete and return account info on success.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "device_name": {
                                    "type": "string",
                                    "description": "Name for this linked device (e.g. 'my-cli', 'work-laptop')"
                                },
                                "servers": {
                                    "type": "string",
                                    "description": "Signal server environment: 'production' (default) or 'staging'",
                                    "default": "production"
                                }
                            },
                            "required": ["device_name"]
                        }
                    }
                ]
            }),
        )
    }

    fn handle_tools_call(&self, id: Value, params: Option<Value>) -> JsonRpcResponse {
        let params = match params {
            Some(p) => p,
            None => return JsonRpcResponse::error(id, -32602, "missing params".into()),
        };

        let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

        let result = self.rt.block_on(async {
            let local = tokio::task::LocalSet::new();
            local.run_until(self.call_tool(tool_name, arguments)).await
        });

        match result {
            Ok(value) => JsonRpcResponse::success(
                id,
                json!({
                    "content": [{ "type": "text", "text": value.to_string() }]
                }),
            ),
            Err(e) => JsonRpcResponse::success(
                id,
                json!({
                    "content": [{ "type": "text", "text": format!("Error: {e}") }],
                    "isError": true
                }),
            ),
        }
    }

    async fn call_tool(&self, name: &str, args: Value) -> anyhow::Result<Value> {
        match name {
            "send_message" => {
                let uuid_str = args
                    .get("uuid")
                    .and_then(|v| v.as_str())
                    .context("missing uuid")?;
                let message = args
                    .get("message")
                    .and_then(|v| v.as_str())
                    .context("missing message")?;

                let uuid: uuid::Uuid = uuid_str.parse()?;
                let store = self.open_store().await?;
                let mut manager = Manager::load_registered(store).await?;

                let timestamp = std::time::SystemTime::now()
                    .duration_since(UNIX_EPOCH)?
                    .as_millis() as u64;

                let data_message = DataMessage {
                    body: Some(message.to_string()),
                    timestamp: Some(timestamp),
                    ..Default::default()
                };

                manager
                    .send_message(
                        ServiceId::Aci(uuid.into()),
                        ContentBody::DataMessage(data_message),
                        timestamp,
                    )
                    .await?;

                Ok(json!({ "status": "sent", "timestamp": timestamp }))
            }

            "send_message_by_phone" => {
                let phone_str = args
                    .get("phone_number")
                    .and_then(|v| v.as_str())
                    .context("missing phone_number")?;
                let message = args
                    .get("message")
                    .and_then(|v| v.as_str())
                    .context("missing message")?;

                let phone: presage::libsignal_service::prelude::phonenumber::PhoneNumber =
                    phone_str.parse()?;
                let store = self.open_store().await?;
                let mut manager = Manager::load_registered(store).await?;

                #[cfg(feature = "cdsi")]
                {
                    let resolved = manager
                        .discover_contacts_by_phone_number(vec![phone])
                        .await?;
                    let (_, identity) = resolved.into_iter().next().context("no CDSI result")?;
                    let service_id = identity.context("phone not registered on Signal")?;

                    let timestamp = std::time::SystemTime::now()
                        .duration_since(UNIX_EPOCH)?
                        .as_millis() as u64;

                    let data_message = DataMessage {
                        body: Some(message.to_string()),
                        timestamp: Some(timestamp),
                        ..Default::default()
                    };

                    manager
                        .send_message(
                            service_id,
                            ContentBody::DataMessage(data_message),
                            timestamp,
                        )
                        .await?;

                    Ok(
                        json!({ "status": "sent", "uuid": service_id.service_id_string(), "timestamp": timestamp }),
                    )
                }

                #[cfg(not(feature = "cdsi"))]
                {
                    anyhow::bail!("CDSI feature not enabled")
                }
            }

            "list_contacts" => {
                let store = self.open_store().await?;
                let manager: Manager<SqliteStore, Registered> =
                    Manager::load_registered(store).await?;
                let contacts: Vec<Value> = manager
                    .store()
                    .contacts()
                    .await?
                    .filter_map(Result::ok)
                    .map(|c| {
                        json!({
                            "uuid": c.uuid.to_string(),
                            "name": c.name,
                            "phone_number": c.phone_number.as_ref().map(|p| p.to_string()),
                        })
                    })
                    .collect();
                Ok(json!(contacts))
            }

            "list_groups" => {
                let store = self.open_store().await?;
                let manager: Manager<SqliteStore, Registered> =
                    Manager::load_registered(store).await?;
                let groups: Vec<Value> = ContentsStore::groups(manager.store())
                    .await?
                    .filter_map(Result::ok)
                    .map(|(key, g)| {
                        json!({
                            "master_key": hex::encode(key),
                            "title": g.title,
                            "members": g.members.len(),
                        })
                    })
                    .collect();
                Ok(json!(groups))
            }

            "receive_messages" => {
                let store = self.open_store().await?;
                let mut manager: Manager<SqliteStore, Registered> =
                    Manager::load_registered(store).await?;
                let messages = manager.receive_messages().await?;
                futures::pin_mut!(messages);

                let mut received = Vec::new();
                while let Some(content) = futures::StreamExt::next(&mut messages).await {
                    match content {
                        presage::model::messages::Received::QueueEmpty => break,
                        presage::model::messages::Received::Content(c) => {
                            let sender_id = c.metadata.sender;
                            let sender_uuid = sender_id.service_id_string();

                            // Try to resolve sender name from contacts
                            let sender_name = manager
                                .store()
                                .contact_by_id(&sender_id)
                                .await
                                .ok()
                                .flatten()
                                .filter(|contact| !contact.name.is_empty())
                                .map(|contact| contact.name);

                            let (msg_type, body, attachments_count, group_info) = match &c.body {
                                ContentBody::DataMessage(DataMessage {
                                    body,
                                    attachments,
                                    group_v2,
                                    ..
                                }) => {
                                    let group = group_v2.as_ref().and_then(|g| {
                                        g.master_key
                                            .as_ref()
                                            .map(|k| json!({ "master_key": hex::encode(k) }))
                                    });
                                    ("message", body.clone(), attachments.len(), group)
                                }
                                ContentBody::SynchronizeMessage(presage::proto::SyncMessage {
                                    sent:
                                        Some(presage::proto::sync_message::Sent {
                                            message:
                                                Some(DataMessage {
                                                    body, attachments, ..
                                                }),
                                            ..
                                        }),
                                    ..
                                }) => ("sync_sent", body.clone(), attachments.len(), None),
                                ContentBody::TypingMessage(_) => ("typing", None, 0, None),
                                ContentBody::ReceiptMessage(_) => ("receipt", None, 0, None),
                                ContentBody::CallMessage(_) => ("call", None, 0, None),
                                _ => ("other", None, 0, None),
                            };

                            let mut msg = json!({
                                "timestamp": c.metadata.timestamp,
                                "sender_uuid": sender_uuid,
                                "sender_name": sender_name,
                                "type": msg_type,
                                "body": body,
                                "attachments_count": attachments_count,
                            });

                            if let Some(group) = group_info {
                                msg["group"] = group;
                            }

                            received.push(msg);
                        }
                        _ => {}
                    }
                }
                Ok(json!({
                    "messages": received,
                    "count": received.len(),
                }))
            }

            "whoami" => {
                let store = self.open_store().await?;
                let manager = Manager::load_registered(store).await?;
                let whoami = manager.whoami().await?;
                Ok(json!({
                    "aci": whoami.aci.to_string(),
                }))
            }

            "resolve_phone" => {
                let phone_str = args
                    .get("phone_number")
                    .and_then(|v| v.as_str())
                    .context("missing phone_number")?;

                #[cfg(feature = "cdsi")]
                {
                    let phone: presage::libsignal_service::prelude::phonenumber::PhoneNumber =
                        phone_str.parse()?;
                    let store = self.open_store().await?;
                    let mut manager = Manager::load_registered(store).await?;
                    let resolved = manager
                        .discover_contacts_by_phone_number(vec![phone])
                        .await?;
                    let results: Vec<Value> = resolved
                        .into_iter()
                        .map(|(num, id)| {
                            json!({
                                "phone": num.to_string(),
                                "uuid": id.map(|i| i.service_id_string()),
                            })
                        })
                        .collect();
                    Ok(json!(results))
                }

                #[cfg(not(feature = "cdsi"))]
                {
                    let _ = phone_str;
                    anyhow::bail!("CDSI feature not enabled")
                }
            }

            "list_messages" => {
                let store = self.open_store().await?;
                let manager: Manager<SqliteStore, Registered> =
                    Manager::load_registered(store).await?;

                let contact_uuid = args.get("contact_uuid").and_then(|v| v.as_str());
                let group_key = args.get("group_master_key").and_then(|v| v.as_str());
                let from_ts = args
                    .get("from_timestamp")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;

                let thread = if let Some(uuid_str) = contact_uuid {
                    let service_id = ServiceId::parse_from_service_id_string(uuid_str)
                        .context("invalid UUID format, expected ACI format like '12345678-...'")?;
                    presage::store::Thread::Contact(service_id)
                } else if let Some(key_hex) = group_key {
                    let key_bytes = hex::decode(key_hex)?;
                    let key: [u8; 32] = key_bytes
                        .try_into()
                        .map_err(|_| anyhow::anyhow!("group master key must be 32 bytes"))?;
                    presage::store::Thread::Group(key)
                } else {
                    anyhow::bail!("provide either contact_uuid or group_master_key")
                };

                use presage::store::ContentsStore;
                let mut messages_out = Vec::new();
                for msg in manager
                    .store()
                    .messages(&thread, from_ts..)
                    .await?
                    .filter_map(Result::ok)
                    .take(limit)
                {
                    let sender_uuid = msg.metadata.sender.service_id_string();
                    let sender_name = manager
                        .store()
                        .contact_by_id(&msg.metadata.sender)
                        .await
                        .ok()
                        .flatten()
                        .filter(|c| !c.name.is_empty())
                        .map(|c| c.name);

                    let (msg_type, body) = match &msg.body {
                        ContentBody::DataMessage(DataMessage { body, .. }) => {
                            ("message", body.clone())
                        }
                        ContentBody::SynchronizeMessage(presage::proto::SyncMessage {
                            sent:
                                Some(presage::proto::sync_message::Sent {
                                    message: Some(DataMessage { body, .. }),
                                    ..
                                }),
                            ..
                        }) => ("sync_sent", body.clone()),
                        _ => ("other", None),
                    };

                    messages_out.push(json!({
                        "timestamp": msg.metadata.timestamp,
                        "sender_uuid": sender_uuid,
                        "sender_name": sender_name,
                        "type": msg_type,
                        "body": body,
                    }));
                }

                Ok(json!({
                    "thread": format!("{thread}"),
                    "messages": messages_out,
                    "count": messages_out.len(),
                }))
            }

            "link_device" => {
                let device_name = args
                    .get("device_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("signal-cli-mcp")
                    .to_string();

                let servers_str = args
                    .get("servers")
                    .and_then(|v| v.as_str())
                    .unwrap_or("production");

                let servers: SignalServers = servers_str.parse().map_err(|_| {
                    anyhow::anyhow!("invalid server: use 'production' or 'staging'")
                })?;

                let store = self.open_store().await?;
                let (tx, rx) = futures::channel::oneshot::channel();

                let link_future =
                    Manager::link_secondary_device(store, servers, device_name.clone(), tx);

                // Wait for the provisioning URL first, then wait for linking to complete
                let (link_result, provisioning_url) =
                    futures::future::join(link_future, async { rx.await.ok() }).await;

                // Build response with provisioning info
                let mut response = json!({
                    "status": "linking",
                    "device_name": device_name,
                });

                if let Some(url) = provisioning_url {
                    let url_string = url.to_string();

                    // Generate QR code as text art
                    let qr_text = qr_to_text(&url_string);

                    response = json!({
                        "status": "provisioning_url_generated",
                        "device_name": device_name,
                        "provisioning_url": url_string,
                        "qr_code": qr_text,
                        "instructions": "Show the QR code or URL to the user. They must scan it with their Signal mobile app: Settings > Linked Devices > Link New Device. The linking process is waiting for the scan to complete."
                    });
                }

                // Check if linking succeeded
                match link_result {
                    Ok(manager) => {
                        let whoami = manager.whoami().await;
                        match whoami {
                            Ok(who) => Ok(json!({
                                "status": "linked",
                                "device_name": device_name,
                                "aci": who.aci.to_string(),
                                "message": format!("Successfully linked as '{device_name}'! Account UUID: {}", who.aci)
                            })),
                            Err(_) => Ok(json!({
                                "status": "linked",
                                "device_name": device_name,
                                "message": format!("Successfully linked as '{device_name}'!")
                            })),
                        }
                    }
                    Err(e) => {
                        // If we got a provisioning URL, include it in the error so the user
                        // can still try scanning
                        if response.get("provisioning_url").is_some() {
                            response["status"] = json!("waiting_for_scan");
                            response["note"] = json!(format!(
                                "The provisioning URL was generated but linking hasn't completed yet. \
                                 The user needs to scan the QR code. Error details: {e}"
                            ));
                            Ok(response)
                        } else {
                            Err(anyhow::anyhow!("linking failed: {e}"))
                        }
                    }
                }
            }

            _ => anyhow::bail!("unknown tool: {name}"),
        }
    }
}

/// Convert a URL string to a QR code rendered as Unicode text
fn qr_to_text(data: &str) -> String {
    use std::fmt::Write;

    // Use the qr2term crate's underlying qr code generation
    let qr = match qrcode::QrCode::new(data.as_bytes()) {
        Ok(qr) => qr,
        Err(_) => return format!("(Could not generate QR code. Use URL directly: {data})"),
    };

    let colors = qr.to_colors();
    let width = qr.width();
    let mut result = String::new();

    // Unicode block elements for compact QR rendering
    // Each character represents 2 vertical pixels
    for y in (0..width).step_by(2) {
        for x in 0..width {
            let top = colors[y * width + x];
            let bottom = if y + 1 < width {
                colors[(y + 1) * width + x]
            } else {
                qrcode::Color::Light
            };

            let ch = match (top, bottom) {
                (qrcode::Color::Dark, qrcode::Color::Dark) => '\u{2588}', // Full block
                (qrcode::Color::Dark, qrcode::Color::Light) => '\u{2580}', // Upper half
                (qrcode::Color::Light, qrcode::Color::Dark) => '\u{2584}', // Lower half
                (qrcode::Color::Light, qrcode::Color::Light) => ' ',      // Empty
            };
            let _ = write!(result, "{ch}");
        }
        result.push('\n');
    }

    result
}

fn main() -> anyhow::Result<()> {
    let filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(tracing::metadata::LevelFilter::WARN.into())
        .from_env_lossy();
    tracing_subscriber::fmt::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .init();

    let server = McpServer::new()?;
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let request: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let resp = JsonRpcResponse::error(Value::Null, -32700, format!("parse error: {e}"));
                let out = serde_json::to_string(&resp)?;
                writeln!(stdout, "{out}")?;
                stdout.flush()?;
                continue;
            }
        };

        let response = server.handle_request(request);
        let out = serde_json::to_string(&response)?;
        writeln!(stdout, "{out}")?;
        stdout.flush()?;
    }

    Ok(())
}
