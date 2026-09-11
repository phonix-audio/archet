//! What a host resolves a plugin by.

/// The four strings a host stores to find a plugin again. Each one is a wire
/// format from the moment anything ships: the class id goes into every
/// project and every `.vstpreset`, the CLAP id identifies the plugin to a
/// CLAP host, and the vendor and name compose the directory a host indexes
/// presets under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    pub name: &'static str,
    pub vendor: &'static str,
    /// Exactly sixteen ASCII bytes; a host hex-encodes them into a FUID.
    pub class_id: [u8; 16],
    pub clap_id: &'static str,
    /// The key the plugin persists its state under.
    pub persist_key: &'static str,
}

impl Identity {
    /// The class id as the uppercase hex a `.vstpreset` header carries.
    pub fn class_id_hex(&self) -> String {
        self.class_id.iter().map(|b| format!("{b:02X}")).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hex_spelling_is_uppercase_and_thirty_two_wide() {
        let id = Identity {
            name: "X", vendor: "Y", class_id: *b"PxCordisPiano001", clap_id: "z", persist_key: "patch",
        };
        assert_eq!(id.class_id_hex(), "5078436F726469735069616E6F303031");
    }
}
