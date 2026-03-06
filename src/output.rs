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
            println!("{}", serde_json::to_string_pretty(value).unwrap_or_default());
        }
    }
}

#[derive(Serialize)]
pub struct JsonMessage {
    pub timestamp: u64,
    pub sender: String,
    pub thread: String,
    pub body: Option<String>,
    pub r#type: String,
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
        )
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
        write!(
            f,
            "{} / {:?} / {}",
            self.uuid, self.phone_number, self.name
        )
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
            self.master_key,
            self.title,
            self.description,
            self.revision,
            self.members_count,
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
        let current = if self.is_current { " (this device)" } else { "" };
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
