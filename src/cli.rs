use std::path::PathBuf;

use clap::{Parser, Subcommand};
use presage::libsignal_service::configuration::SignalServers;
use presage::libsignal_service::prelude::phonenumber::PhoneNumber;
use presage::libsignal_service::prelude::Uuid;
use presage::libsignal_service::protocol::ServiceId;
use presage::libsignal_service::zkgroup::GroupMasterKeyBytes;
use url::Url;

use crate::output::OutputFormat;

fn parse_service_id(input: &str) -> Result<ServiceId, clap::Error> {
    ServiceId::parse_from_service_id_string(input)
        .ok_or_else(|| clap::Error::new(clap::error::ErrorKind::InvalidValue))
}

fn parse_group_master_key(value: &str) -> anyhow::Result<GroupMasterKeyBytes> {
    let bytes = hex::decode(value)?;
    bytes
        .try_into()
        .map_err(|_| anyhow::format_err!("master key should be 32 bytes long"))
}

#[derive(Parser)]
#[clap(name = "signal-cli", about = "Lightweight Signal CLI in Rust", version)]
pub struct Args {
    #[clap(long = "db-path", help = "Path to SQLite database")]
    pub db_path: Option<String>,

    #[clap(long = "passphrase", short = 'p', help = "Passphrase to encrypt local storage")]
    pub passphrase: Option<String>,

    #[clap(long = "output", short = 'o', default_value = "text", help = "Output format")]
    pub output_format: OutputFormat,

    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    #[clap(about = "Register a new account with a phone number")]
    Register {
        #[clap(long, short = 's', default_value = "production")]
        servers: SignalServers,
        #[clap(long, help = "Phone number in E.164 format (e.g. +14154380765)")]
        phone_number: PhoneNumber,
        #[clap(long)]
        use_voice_call: bool,
        #[clap(long, help = "Captcha from https://signalcaptchas.org/registration/generate.html")]
        captcha: Url,
        #[clap(long, help = "Force re-registration")]
        force: bool,
    },

    #[clap(about = "Link as a secondary device (generates QR code)")]
    LinkDevice {
        #[clap(long, short = 's', default_value = "production")]
        servers: SignalServers,
        #[clap(long, short = 'n', help = "Device name")]
        device_name: String,
    },

    #[clap(about = "Send a message to a contact")]
    Send {
        #[clap(long, short = 'u', help = "Recipient UUID")]
        uuid: Option<Uuid>,
        #[clap(long, short = 'n', help = "Recipient phone number (requires CDSI)")]
        phone_number: Option<PhoneNumber>,
        #[clap(long, short = 'm', help = "Message text")]
        message: String,
        #[clap(long = "attach", help = "File paths to attach")]
        attachment: Vec<PathBuf>,
    },

    #[clap(about = "Send a message to a group")]
    SendToGroup {
        #[clap(long, short = 'k', help = "Group master key (hex)", value_parser = parse_group_master_key)]
        master_key: GroupMasterKeyBytes,
        #[clap(long, short = 'm', help = "Message text")]
        message: String,
        #[clap(long = "attach", help = "File paths to attach")]
        attachment: Vec<PathBuf>,
    },

    #[clap(about = "Receive and display messages")]
    Receive {
        #[clap(long, short = 'n', help = "Show desktop notifications")]
        notifications: bool,
        #[clap(long, short = 'q', help = "Exit after queue is empty")]
        stop_after_empty_queue: bool,
    },

    #[clap(about = "List all groups")]
    ListGroups {
        #[clap(long, help = "Filter groups by name")]
        name_filter: Option<String>,
    },

    #[clap(about = "List all contacts")]
    ListContacts,

    #[clap(about = "List messages in a thread")]
    ListMessages {
        #[clap(long, short = 'u', help = "Recipient service ID", value_parser = parse_service_id)]
        recipient: Option<ServiceId>,
        #[clap(long, short = 'k', help = "Group master key (hex)", value_parser = parse_group_master_key)]
        group: Option<GroupMasterKeyBytes>,
        #[clap(long, help = "Start from UNIX timestamp")]
        from: Option<u64>,
    },

    #[clap(about = "Show account info")]
    Whoami,

    #[clap(about = "List linked devices")]
    ListDevices,

    #[clap(about = "Resolve phone numbers to UUIDs")]
    #[cfg(feature = "cdsi")]
    Resolve {
        #[clap(long, short = 'n', help = "Phone numbers to resolve")]
        phone_number: Vec<PhoneNumber>,
    },

    #[clap(about = "Sync contacts from primary device")]
    SyncContacts,

    #[clap(about = "Retrieve a user profile")]
    GetProfile {
        #[clap(long, help = "UUID of the user")]
        uuid: Uuid,
    },

    #[clap(about = "Show protocol statistics")]
    Stats,
}
