use serde::Serialize;
use std::fmt;

#[derive(Clone, Debug, Default)]
pub enum OutputFormat {
    #[default]
    Text,
    Json,
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OutputFormat::Text => write!(f, "text"),
            OutputFormat::Json => write!(f, "json"),
        }
    }
}

impl std::str::FromStr for OutputFormat {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "text" | "plain" => Ok(OutputFormat::Text),
            "json" => Ok(OutputFormat::Json),
            _ => Err(format!("unknown output format: {s}")),
        }
    }
}

pub fn print_result<T: Serialize + fmt::Display>(format: &OutputFormat, value: &T) {
    match format {
        OutputFormat::Text => println!("{value}"),
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(value).unwrap_or_default()
            );
        }
    }
}

/// An attachment `receive` saved, reported with the message it came in.
#[derive(Serialize)]
pub struct JsonAttachment {
    /// Where it was saved: a name this CLI chose (`<timestamp>-<n>.<ext>`) inside its own
    /// attachments directory. Never derived from anything the sender wrote.
    pub path: String,
    /// The sender's stated type, as received.
    pub content_type: Option<String>,
    /// The sender's name for the file, as DATA only: it is never used to build a path.
    pub file_name: Option<String>,
    /// Bytes saved.
    pub size: u64,
    /// The sender's app recorded it as a voice message (AttachmentPointer.Flags.VOICE_MESSAGE).
    pub voice_note: bool,
}

#[derive(Serialize)]
pub struct JsonMessage {
    pub timestamp: u64,
    pub sender: String,
    pub thread: String,
    pub body: Option<String>,
    pub r#type: String,
    /// Left out when there are none, so a message without attachments prints as before.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<JsonAttachment>,
}

impl fmt::Display for JsonMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}] {} -> {}: {}",
            self.timestamp,
            self.sender,
            self.thread,
            self.body.as_deref().unwrap_or("")
        )?;
        for a in &self.attachments {
            write!(
                f,
                "\n  attachment{}: {} ({}, {} bytes)",
                if a.voice_note { " (voice message)" } else { "" },
                a.path,
                a.content_type.as_deref().unwrap_or("unknown type"),
                a.size
            )?;
        }
        Ok(())
    }
}

#[derive(Serialize)]
pub struct JsonContact {
    pub uuid: String,
    pub phone_number: Option<String>,
    pub name: String,
}

impl fmt::Display for JsonContact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} / {:?} / {}", self.uuid, self.phone_number, self.name)
    }
}

#[derive(Serialize)]
pub struct JsonGroup {
    pub master_key: String,
    pub title: String,
    pub description: Option<String>,
    pub members_count: usize,
    pub revision: u32,
}

impl fmt::Display for JsonGroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {}: {:?} / revision {} / {} members",
            self.master_key, self.title, self.description, self.revision, self.members_count,
        )
    }
}

#[derive(Serialize)]
pub struct JsonDevice {
    pub id: u32,
    pub name: Option<String>,
    pub created: String,
    pub last_seen: String,
    pub is_current: bool,
}

impl fmt::Display for JsonDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let current = if self.is_current {
            " (this device)"
        } else {
            ""
        };
        write!(
            f,
            "- Device {}{}\n  Name: {}\n  Created: {}\n  Last seen: {}",
            self.id,
            current,
            self.name.as_deref().unwrap_or("(no name)"),
            self.created,
            self.last_seen,
        )
    }
}
