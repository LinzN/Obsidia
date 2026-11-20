//! Phosphor icon font (MIT, https://phosphoricons.com), reduced to the icon
//! code points so it can sit in front of the text font. Used as a fallback
//! font so icons can be written straight into labels.

pub const FONT: &[u8] = include_bytes!("../assets/fonts/Phosphor.ttf");

pub const GEAR: &str = "\u{E270}";
pub const GITHUB_LOGO: &str = "\u{E576}";
pub const GLOBE: &str = "\u{E288}";
pub const INFO: &str = "\u{E2CE}";
pub const SEAL_CHECK: &str = "\u{E606}";
