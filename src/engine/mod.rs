//! UI-independent parts of the reader: page sources, layout rules, image
//! processing and settings storage. Nothing in here may depend on egui, so
//! it can be tested headless and reused by a different frontend.

pub mod history;
pub mod imaging;
pub mod layout;
pub mod library;
pub mod settings_store;
pub mod source;
