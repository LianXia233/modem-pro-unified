//! `GET /api/pcie` —— PCIe（MHI / wwan）模组清单 + 本平台能力。

use std::collections::HashMap;

use serde_json::json;
use tiny_http::Request;

use crate::error::Result;
use crate::http::{response, AppState};
use crate::modem::pcidev;

pub fn handle(req: Request, _state: &AppState, _query: &HashMap<String, String>) -> Result<()> {
    response::ok(
        req,
        json!({
            "supported": pcidev::reset_supported(),
            "drivers": pcidev::PCIE_MODEM_DRIVERS,
            "devices": pcidev::list_pci_modem_devices(),
            "at_ports": pcidev::pcie_at_port_candidates(),
            "qmi_ctrl": pcidev::pcie_qmi_control_candidates(),
            "mbim_dev": pcidev::pcie_mbim_candidates(),
            "rescan": pcidev::PCI_RESCAN_PATH,
        }),
    )
}
