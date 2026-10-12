//! Device identification structures. Port of `gomodbus/common/device.go`.

use super::types::*;

/// DeviceIdObject represents a single device identification object.
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (Read Device Identification - Response PDU, object list)
#[derive(Debug, Clone, PartialEq, Eq, Default)]
/// NOTE: `value` is a UTF-8 string; non-UTF-8 wire bytes are decoded lossily (U+FFFD),
/// whereas Go keeps raw bytes. `length` always holds the original wire byte length.
pub struct DeviceIdObject {
    /// Object ID - Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (object Id list: 0x00-0x02 Basic, 0x03-0x06 Regular, 0x80-0xFF private)
    pub id: DeviceIdObjectCode,
    /// Length of the object value in bytes - Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (Response PDU object format)
    pub length: u8,
    /// Object value.
    pub value: String,
}

/// DeviceIdentification represents a complete device identification response.
///
/// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (Read Device Identification - Response PDU)
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeviceIdentification {
    /// Echoes request - Ref: Section 6.21, Response PDU
    pub read_device_id_code: ReadDeviceIdCode,
    /// Conformity level of the device - Ref: Section 6.21 (Response parameters: conformity level)
    pub conformity_level: ConformityLevel,
    /// 0x00 = No, 0xFF = Yes - Ref: Section 6.21, Response PDU
    pub more_follows: MoreFollows,
    /// Object ID to request next if `more_follows` is yes - Ref: Section 6.21, Response PDU
    pub next_object_id: DeviceIdObjectCode,
    /// Number of identification objects in this response part - Ref: Section 6.21, Response PDU
    pub number_of_objects: u8,
    /// The list of device identification objects.
    pub objects: Vec<DeviceIdObject>,
}

impl DeviceIdentification {
    /// Returns the object with the specified ID, or `None` if not found.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (object Id list: 0x00-0x02 Basic, 0x03-0x06 Regular, 0x80-0xFF private)
    /// Used to retrieve device identification objects by their ID code.
    pub fn get_object(&self, id: DeviceIdObjectCode) -> Option<&DeviceIdObject> {
        self.objects.iter().find(|o| o.id == id)
    }

    // Value of the object with `id`, or "" when absent (shared by the getters below).
    fn value_of(&self, id: DeviceIdObjectCode) -> &str {
        self.get_object(id).map(|o| o.value.as_str()).unwrap_or("")
    }

    /// Returns the vendor name (object ID 0x00), or "" if not present.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (object Id list: 0x00-0x02 Basic, 0x03-0x06 Regular, 0x80-0xFF private)
    /// This is a mandatory basic identification object and should always be present.
    pub fn get_vendor_name(&self) -> &str {
        self.value_of(DEVICE_ID_VENDOR_NAME)
    }

    /// Returns the product code (object ID 0x01), or "" if not present.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (object Id list: 0x00-0x02 Basic, 0x03-0x06 Regular, 0x80-0xFF private)
    /// This is a mandatory basic identification object and should always be present.
    pub fn get_product_code(&self) -> &str {
        self.value_of(DEVICE_ID_PRODUCT_CODE)
    }

    /// Returns the major/minor revision (object ID 0x02), or "" if not present.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (object Id list: 0x00-0x02 Basic, 0x03-0x06 Regular, 0x80-0xFF private)
    /// This is a mandatory basic identification object and should always be present.
    pub fn get_revision(&self) -> &str {
        self.value_of(DEVICE_ID_MAJOR_MINOR_REVISION)
    }

    /// Returns the vendor URL (object ID 0x03), or "" if not present.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (object Id list: 0x00-0x02 Basic, 0x03-0x06 Regular, 0x80-0xFF private)
    /// This is an optional regular identification object.
    pub fn get_vendor_url(&self) -> &str {
        self.value_of(DEVICE_ID_VENDOR_URL)
    }

    /// Returns the product name (object ID 0x04), or "" if not present.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (object Id list: 0x00-0x02 Basic, 0x03-0x06 Regular, 0x80-0xFF private)
    /// This is an optional regular identification object.
    pub fn get_product_name(&self) -> &str {
        self.value_of(DEVICE_ID_PRODUCT_NAME)
    }

    /// Returns the model name (object ID 0x05), or "" if not present.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (object Id list: 0x00-0x02 Basic, 0x03-0x06 Regular, 0x80-0xFF private)
    /// This is an optional regular identification object.
    pub fn get_model_name(&self) -> &str {
        self.value_of(DEVICE_ID_MODEL_NAME)
    }

    /// Returns the user application name (object ID 0x06), or "" if not present.
    /// Ref: Modbus_Application_Protocol_V1_1b3.pdf, Section 6.21 (object Id list: 0x00-0x02 Basic, 0x03-0x06 Regular, 0x80-0xFF private)
    /// This is an optional regular identification object.
    pub fn get_user_application_name(&self) -> &str {
        self.value_of(DEVICE_ID_USER_APP_NAME)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn getters() {
        let d = DeviceIdentification {
            objects: vec![
                DeviceIdObject { id: DEVICE_ID_VENDOR_NAME, length: 1, value: "V".into() },
                DeviceIdObject { id: DEVICE_ID_MODEL_NAME, length: 1, value: "M".into() },
            ],
            ..Default::default()
        };
        assert_eq!(d.get_vendor_name(), "V");
        assert_eq!(d.get_model_name(), "M");
        assert_eq!(d.get_product_code(), "");
        assert!(d.get_object(DEVICE_ID_USER_APP_NAME).is_none());
    }
}
