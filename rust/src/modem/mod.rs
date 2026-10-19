//! 模组领域：AT 收发 / 模型 / 发现 / 厂商 / 清单。

pub mod at;
pub mod atplan;
pub mod bands;
pub mod catalog;
pub mod detect;
mod discovery;
pub mod manager;
pub mod metrics;
pub mod model;
pub mod pcidev;
pub mod ports;
pub mod quirk;
pub mod usbdev;
pub mod variant;
pub mod vendor;

pub use discovery::{port_exists, ports_list};
pub use manager::ModemManager;
