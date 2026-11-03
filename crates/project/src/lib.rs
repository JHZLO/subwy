//! The rules behind subwy: the folder tree, keyboard moves, copied paths and file I/O.
//! Nothing here depends on GPUI, so these rules test in seconds.

pub mod finder;
pub mod folders;
pub mod fs;
pub mod fuzzy;
pub mod index;
pub mod lines;
pub mod markdown;
pub mod nav;
pub mod network;
pub mod paths;
pub mod search;
pub mod session;
pub mod settings;
pub mod terminal;
pub mod tree;
pub mod watch;
