//! What a monitor says about itself in its EDID block: who made it, which
//! model it is, its serial number and its name. The system's own names for
//! monitors follow the port or the order they were found in; this stays with
//! the monitor itself, which is what lets a show tell that the same monitor
//! has come back.
#![cfg_attr(target_os = "macos", allow(dead_code))]

/// The parts of an EDID block NewQ uses.
#[derive(Clone, Debug, PartialEq)]
pub struct Edid {
    /// Three letters, e.g. "SAM".
    pub manufacturer: String,
    pub product: u16,
    /// The 32-bit serial number; 0 when the maker left it out.
    pub serial: u32,
    /// The name in the monitor's descriptors ("SAMSUNG", "DELL U2415").
    pub name: Option<String>,
    /// The serial number as text, if the descriptors have one.
    pub serial_text: Option<String>,
}

const HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];

/// Read the first 128 bytes of an EDID.
pub fn parse(bytes: &[u8]) -> Option<Edid> {
    if bytes.len() < 128 || bytes[..8] != HEADER {
        return None;
    }
    // Three five-bit letters, A = 1, packed into two bytes.
    let packed = u16::from_be_bytes([bytes[8], bytes[9]]);
    let letter = |shift: u16| {
        let v = ((packed >> shift) & 0x1F) as u8;
        (1..=26).contains(&v).then(|| (b'A' + v - 1) as char)
    };
    let manufacturer: String = [letter(10)?, letter(5)?, letter(0)?].iter().collect();
    let product = u16::from_le_bytes([bytes[10], bytes[11]]);
    let serial = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
    let mut name = None;
    let mut serial_text = None;
    // Four 18-byte descriptors; the ones that start with zeros are text.
    for at in [54, 72, 90, 108] {
        let d = &bytes[at..at + 18];
        if d[..3] != [0, 0, 0] {
            continue;
        }
        let text = || {
            let t: String = d[5..18]
                .iter()
                .take_while(|b| **b != 0x0A)
                .map(|b| *b as char)
                .filter(|c| !c.is_control())
                .collect();
            let t = t.trim().to_string();
            (!t.is_empty()).then_some(t)
        };
        match d[3] {
            0xFC => name = name.or_else(text),
            0xFF => serial_text = serial_text.or_else(text),
            _ => {}
        }
    }
    Some(Edid {
        manufacturer,
        product,
        serial,
        name,
        serial_text,
    })
}

impl Edid {
    /// What stays the same for this monitor: its make and model, and its
    /// serial number when it has one. Two monitors of one model without
    /// serial numbers share an id; the show tells those apart by order.
    pub fn hardware_id(&self) -> String {
        let model = format!("{}{:04X}", self.manufacturer, self.product);
        match (&self.serial_text, self.serial) {
            (Some(s), _) => format!("{model}:{s}"),
            (None, 0) => model,
            (None, n) => format!("{model}:{n:08X}"),
        }
    }

    /// What to call it: its name, else its make and model.
    pub fn title(&self) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| format!("{} {:04X}", self.manufacturer, self.product))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// An EDID block as a monitor would hold it.
    pub(crate) fn sample(
        mfg: &str,
        product: u16,
        serial: u32,
        name: Option<&str>,
        serial_text: Option<&str>,
    ) -> Vec<u8> {
        let mut b = vec![0u8; 128];
        b[..8].copy_from_slice(&HEADER);
        let l = |c: char| (c as u16 - 'A' as u16 + 1) & 0x1F;
        let chars: Vec<char> = mfg.chars().collect();
        let packed = (l(chars[0]) << 10) | (l(chars[1]) << 5) | l(chars[2]);
        b[8..10].copy_from_slice(&packed.to_be_bytes());
        b[10..12].copy_from_slice(&product.to_le_bytes());
        b[12..16].copy_from_slice(&serial.to_le_bytes());
        let mut put = |at: usize, tag: u8, text: &str| {
            b[at + 3] = tag;
            let t = format!("{text}\n");
            for (i, c) in t
                .bytes()
                .chain(std::iter::repeat(b' '))
                .take(13)
                .enumerate()
            {
                b[at + 5 + i] = c;
            }
        };
        if let Some(n) = name {
            put(54, 0xFC, n);
        }
        if let Some(s) = serial_text {
            put(72, 0xFF, s);
        }
        b
    }

    #[test]
    fn reads_who_made_it_which_model_and_its_name() {
        let e = parse(&sample("SAM", 0x0F6B, 0x0001_2345, Some("SAMSUNG"), None)).unwrap();
        assert_eq!(e.manufacturer, "SAM");
        assert_eq!((e.product, e.serial), (0x0F6B, 0x0001_2345));
        assert_eq!(e.name.as_deref(), Some("SAMSUNG"));
        assert_eq!(e.title(), "SAMSUNG");
        assert_eq!(e.hardware_id(), "SAM0F6B:00012345");
    }

    #[test]
    fn the_serial_text_wins_over_the_number_and_no_serial_means_model_only() {
        let text = parse(&sample(
            "DEL",
            0xA0F3,
            7,
            Some("DELL U2415"),
            Some("7MT0187S1HAB"),
        ))
        .unwrap();
        assert_eq!(text.hardware_id(), "DEL A0F3:7MT0187S1HAB".replace(' ', ""));
        assert_eq!(text.title(), "DELL U2415");
        // Many monitors leave the serial out: identical ones then share an id.
        let none = parse(&sample("LGD", 0x0509, 0, None, None)).unwrap();
        assert_eq!(none.hardware_id(), "LGD0509");
        assert_eq!(none.title(), "LGD 0509", "no name: make and model");
    }

    #[test]
    fn things_that_are_not_an_edid_are_not_read() {
        assert_eq!(parse(&[]), None);
        assert_eq!(parse(&[0u8; 128]), None, "no header");
        let mut short = sample("SAM", 1, 1, None, None);
        short.truncate(100);
        assert_eq!(parse(&short), None);
        // Letters outside A to Z mean it isn't one.
        let mut bad = sample("SAM", 1, 1, None, None);
        bad[8] = 0;
        bad[9] = 0;
        assert_eq!(parse(&bad), None);
    }
}
