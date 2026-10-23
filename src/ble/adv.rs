//! Advertising and scan response data builder.
//!
//! ```ignore
//! let adv = AdvData::new()
//!     .flags(GAP_ADTYPE_FLAGS_GENERAL | GAP_ADTYPE_FLAGS_BREDR_NOT_SUPPORTED)
//!     .uuid16_list(&[0xFFE0])
//!     .short_name("dev");
//! let scan_rsp = AdvData::new().name("device");
//! // ble::peripheral::start(...) takes these, or set them directly (the library copies):
//! let data = adv.as_slice();
//! GAPRole_SetParameter(GAPROLE_ADVERT_DATA, data.len() as u16, data.as_ptr() as _)?;
//! ```

use heapless::Vec;

use super::gap::*;

/// Maximum advertising / scan response data length (legacy advertising)
pub const MAX_ADV_LEN: usize = 31;

/// Advertising or scan response data, built from AD structures.
///
/// Panics if the data exceeds 31 bytes (a configuration error).
#[derive(Clone)]
pub struct AdvData(Vec<u8, MAX_ADV_LEN>);

impl AdvData {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// Add an AD structure: length, type, data
    pub fn add(mut self, ad_type: u8, data: &[u8]) -> Self {
        self.0.push(data.len() as u8 + 1).expect("AdvData too long");
        self.0.push(ad_type).expect("AdvData too long");
        self.0.extend_from_slice(data).expect("AdvData too long");
        self
    }

    /// Flags, e.g. `GAP_ADTYPE_FLAGS_GENERAL | GAP_ADTYPE_FLAGS_BREDR_NOT_SUPPORTED`
    pub fn flags(self, flags: u8) -> Self {
        self.add(GAP_ADTYPE_FLAGS, &[flags])
    }

    /// Complete local name
    pub fn name(self, name: &str) -> Self {
        self.add(GAP_ADTYPE_LOCAL_NAME_COMPLETE, name.as_bytes())
    }

    /// Shortened local name (should be the start of the complete name), e.g.
    /// in the advertising packet when the complete name is in the scan response
    pub fn short_name(self, name: &str) -> Self {
        self.add(GAP_ADTYPE_LOCAL_NAME_SHORT, name.as_bytes())
    }

    /// Complete list of 16-bit service UUIDs
    pub fn uuid16_list(self, uuids: &[u16]) -> Self {
        let mut data: Vec<u8, MAX_ADV_LEN> = Vec::new();
        for uuid in uuids {
            data.extend_from_slice(&uuid.to_le_bytes()).expect("AdvData too long");
        }
        self.add(GAP_ADTYPE_16BIT_COMPLETE, &data)
    }

    /// Preferred connection interval range (1.25ms units)
    pub fn conn_interval_range(self, min: u16, max: u16) -> Self {
        let [a, b] = min.to_le_bytes();
        let [c, d] = max.to_le_bytes();
        self.add(GAP_ADTYPE_SLAVE_CONN_INTERVAL_RANGE, &[a, b, c, d])
    }

    /// TX power level (dBm)
    pub fn tx_power(self, dbm: i8) -> Self {
        self.add(GAP_ADTYPE_POWER_LEVEL, &[dbm as u8])
    }

    /// Manufacturer specific data: company ID and payload
    pub fn manufacturer(self, company_id: u16, data: &[u8]) -> Self {
        let mut payload: Vec<u8, MAX_ADV_LEN> = Vec::new();
        payload.extend_from_slice(&company_id.to_le_bytes()).expect("AdvData too long");
        payload.extend_from_slice(data).expect("AdvData too long");
        self.add(GAP_ADTYPE_MANUFACTURER_SPECIFIC, &payload)
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

impl Default for AdvData {
    fn default() -> Self {
        Self::new()
    }
}
