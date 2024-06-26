use lazy_static::lazy_static;
use regex::Regex;

pub const CONFIG_FILE_NAME: &str = "config.json";
pub const EXTRACTOR_DIR: &str = "dofus/datafus";
pub const DATA_URL: &str = "https://github.com/Vahor/Datafus/releases";
pub const EVENTS_FILE: &str = "events.json";

lazy_static! {
    pub static ref VERSION_REGEX: Regex = Regex::new(r"(\d+\.\d+\.\d+)").unwrap();
}
