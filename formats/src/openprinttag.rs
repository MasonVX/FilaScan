use alloc::{string::String, vec::Vec};

use minicbor::{Decoder, data::Type};
use sha1::{Digest, Sha1};

pub const MIME_TYPE: &[u8] = b"application/vnd.openprinttag";
pub const MAX_SECTION_SIZE: usize = 512;
const INSTANCE_UUID_NAMESPACE: [u8; 16] = [
    0x31, 0x06, 0x2f, 0x81, 0xb5, 0xbd, 0x4f, 0x86, 0xa5, 0xf8, 0x46, 0x36, 0x7e, 0x84, 0x15, 0x08,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub alpha: u8,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TemperatureRange {
    pub minimum_celsius: Option<f64>,
    pub maximum_celsius: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenPrintTag {
    pub ndef_uri: Option<String>,
    pub instance_uuid: Option<[u8; 16]>,
    pub package_uuid: Option<[u8; 16]>,
    pub material_uuid: Option<[u8; 16]>,
    pub brand_uuid: Option<[u8; 16]>,
    pub gtin: Option<u64>,
    pub brand_specific_instance_id: Option<String>,
    pub brand_specific_package_id: Option<String>,
    pub brand_specific_material_id: Option<String>,
    pub material_class: u16,
    pub material_type: Option<u16>,
    pub material_name: Option<String>,
    pub material_abbreviation: Option<String>,
    pub brand_name: Option<String>,
    pub nominal_weight_g: Option<f64>,
    pub actual_weight_g: Option<f64>,
    pub empty_container_weight_g: Option<f64>,
    pub nominal_length_mm: Option<f64>,
    pub actual_length_mm: Option<f64>,
    pub filament_diameter_um: Option<f64>,
    pub minimum_nozzle_diameter_um: Option<f64>,
    pub primary_color: Option<Color>,
    pub secondary_colors: Vec<Color>,
    pub nozzle_temperature: TemperatureRange,
    pub bed_temperature: TemperatureRange,
    pub drying_temperature_celsius: Option<f64>,
    pub drying_time_minutes: Option<f64>,
    pub consumed_weight_g: Option<f64>,
    pub storage_location: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    MissingNdefTlv,
    InvalidTlv,
    InvalidNdef,
    ChunkedRecord,
    MissingOpenPrintTagRecord,
    InvalidCbor,
    InvalidMetaRegion,
    InvalidMainRegion,
    InvalidAuxRegion,
    MissingMaterialClass,
}

#[derive(Debug, Clone, Copy)]
struct Regions {
    main_offset: usize,
    main_size: usize,
    aux: Option<(usize, usize)>,
}

pub fn decode_tag_memory(memory: &[u8]) -> Result<OpenPrintTag, Error> {
    let ndef = find_ndef_message(memory)?;
    let payload = find_openprinttag_payload(ndef)?;
    let mut tag = decode_payload(payload)?;
    tag.ndef_uri = find_ndef_uri(ndef);
    Ok(tag)
}

pub fn decode_payload(payload: &[u8]) -> Result<OpenPrintTag, Error> {
    let regions = decode_regions(payload)?;
    let main_end = regions
        .main_offset
        .checked_add(regions.main_size)
        .filter(|end| *end <= payload.len())
        .ok_or(Error::InvalidMainRegion)?;
    let main = &payload[regions.main_offset..main_end];
    let mut tag = decode_main(main)?;

    if let Some((offset, size)) = regions.aux {
        let end = offset
            .checked_add(size)
            .filter(|end| *end <= payload.len())
            .ok_or(Error::InvalidAuxRegion)?;
        decode_aux(&payload[offset..end], &mut tag)?;
    }

    Ok(tag)
}

pub fn derive_instance_uuid(nfc_v_uid: &[u8; 8]) -> [u8; 16] {
    let digest = Sha1::new()
        .chain_update(INSTANCE_UUID_NAMESPACE)
        .chain_update(nfc_v_uid)
        .finalize();
    let mut uuid = [0_u8; 16];
    uuid.copy_from_slice(&digest[..16]);
    uuid[6] = (uuid[6] & 0x0f) | 0x50;
    uuid[8] = (uuid[8] & 0x3f) | 0x80;
    uuid
}

pub const fn material_type_abbreviation(key: u16) -> Option<&'static str> {
    Some(match key {
        0 => "PLA", 1 => "PETG", 2 => "TPU", 3 => "ABS", 4 => "ASA", 5 => "PC", 6 => "PCTG", 7 => "PP",
        8 => "PA6", 9 => "PA11", 10 => "PA12", 11 => "PA66", 12 => "CPE", 13 => "TPE", 14 => "HIPS", 15 => "PHA",
        16 => "PET", 17 => "PEI", 18 => "PBT", 19 => "PVB", 20 => "PVA", 21 => "PEKK", 22 => "PEEK", 23 => "BVOH",
        24 => "TPC", 25 => "PPS", 26 => "PPSU", 27 => "PVC", 28 => "PEBA", 29 => "PVDF", 30 => "PPA", 31 => "PCL",
        32 => "PES", 33 => "PMMA", 34 => "POM", 35 => "PPE", 36 => "PS", 37 => "PSU", 38 => "TPI", 39 => "SBS",
        40 => "OBC", 41 => "EVA", 42 => "PA612",
        _ => return None,
    })
}

fn find_ndef_message(memory: &[u8]) -> Result<&[u8], Error> {
    let mut cursor = match memory.first() {
        Some(0xe1) if memory.len() >= 4 => 4,
        Some(0xe2) if memory.len() >= 8 => 8,
        _ => return Err(Error::MissingNdefTlv),
    };
    while cursor < memory.len() {
        let tag = memory[cursor];
        cursor += 1;
        match tag {
            0x00 => continue,
            0xfe => break,
            _ => {
                let (length, length_size) = if memory.get(cursor) == Some(&0xff) {
                    let bytes = memory.get(cursor + 1..cursor + 3).ok_or(Error::InvalidTlv)?;
                    (u16::from_be_bytes([bytes[0], bytes[1]]) as usize, 3)
                } else {
                    (*memory.get(cursor).ok_or(Error::InvalidTlv)? as usize, 1)
                };
                cursor = cursor.checked_add(length_size).ok_or(Error::InvalidTlv)?;
                let end = cursor.checked_add(length).filter(|end| *end <= memory.len()).ok_or(Error::InvalidTlv)?;
                if tag == 0x03 {
                    return Ok(&memory[cursor..end]);
                }
                cursor = end;
            }
        }
    }
    Err(Error::MissingNdefTlv)
}

fn find_openprinttag_payload(message: &[u8]) -> Result<&[u8], Error> {
    let mut cursor = 0;
    while cursor < message.len() {
        let header = *message.get(cursor).ok_or(Error::InvalidNdef)?;
        cursor += 1;
        let chunked = header & 0x20 != 0;
        let short = header & 0x10 != 0;
        let has_id = header & 0x08 != 0;
        let tnf = header & 0x07;
        if chunked {
            return Err(Error::ChunkedRecord);
        }

        let type_length = *message.get(cursor).ok_or(Error::InvalidNdef)? as usize;
        cursor += 1;
        let payload_length = if short {
            let value = *message.get(cursor).ok_or(Error::InvalidNdef)? as usize;
            cursor += 1;
            value
        } else {
            let bytes = message.get(cursor..cursor + 4).ok_or(Error::InvalidNdef)?;
            cursor += 4;
            u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize
        };
        let id_length = if has_id {
            let value = *message.get(cursor).ok_or(Error::InvalidNdef)? as usize;
            cursor += 1;
            value
        } else {
            0
        };

        let record_end = cursor
            .checked_add(type_length)
            .and_then(|value| value.checked_add(id_length))
            .and_then(|value| value.checked_add(payload_length))
            .filter(|end| *end <= message.len())
            .ok_or(Error::InvalidNdef)?;
        let record_type = &message[cursor..cursor + type_length];
        cursor += type_length + id_length;
        let record_payload = &message[cursor..cursor + payload_length];
        if tnf == 0x02 && record_type == MIME_TYPE {
            return Ok(record_payload);
        }
        cursor = record_end;
        if header & 0x40 != 0 {
            break;
        }
    }
    Err(Error::MissingOpenPrintTagRecord)
}

fn decode_regions(payload: &[u8]) -> Result<Regions, Error> {
    let mut decoder = Decoder::new(payload);
    let count = decoder.map().map_err(|_| Error::InvalidCbor)?;
    let mut main_offset = None;
    let mut main_size = None;
    let mut aux_offset = None;
    let mut aux_size = None;
    visit_map(&mut decoder, count, |decoder, key| {
        match key {
            0 => main_offset = Some(decode_usize(decoder)?),
            1 => main_size = Some(decode_usize(decoder)?),
            2 => aux_offset = Some(decode_usize(decoder)?),
            3 => aux_size = Some(decode_usize(decoder)?),
            _ => decoder.skip().map_err(|_| Error::InvalidCbor)?,
        }
        Ok(())
    })?;
    let meta_end = decoder.position();
    let main_offset = main_offset.unwrap_or(meta_end);
    let next_region = aux_offset.filter(|offset| *offset > main_offset).unwrap_or(payload.len());
    let main_size = main_size.unwrap_or_else(|| next_region.saturating_sub(main_offset));
    if meta_end > MAX_SECTION_SIZE || main_offset < meta_end || main_offset > payload.len() {
        return Err(Error::InvalidMetaRegion);
    }
    let aux = match aux_offset {
        Some(offset) => {
            let next_region = if offset < main_offset { main_offset } else { payload.len() };
            let size = aux_size.unwrap_or_else(|| next_region.saturating_sub(offset));
            if offset < meta_end || offset > payload.len() {
                return Err(Error::InvalidMetaRegion);
            }
            Some((offset, size))
        }
        None => None,
    };
    let main_end = main_offset.checked_add(main_size).ok_or(Error::InvalidMetaRegion)?;
    if main_end > payload.len() {
        return Err(Error::InvalidMetaRegion);
    }
    if let Some((aux_offset, aux_size)) = aux {
        let aux_end = aux_offset.checked_add(aux_size).filter(|end| *end <= payload.len()).ok_or(Error::InvalidMetaRegion)?;
        if main_offset < aux_end && aux_offset < main_end {
            return Err(Error::InvalidMetaRegion);
        }
    }
    Ok(Regions { main_offset, main_size, aux })
}

fn decode_main(data: &[u8]) -> Result<OpenPrintTag, Error> {
    let mut decoder = Decoder::new(data);
    let count = decoder.map().map_err(|_| Error::InvalidCbor)?;
    let mut instance_uuid = None;
    let mut package_uuid = None;
    let mut material_uuid = None;
    let mut brand_uuid = None;
    let mut gtin = None;
    let mut brand_specific_instance_id = None;
    let mut brand_specific_package_id = None;
    let mut brand_specific_material_id = None;
    let mut material_class = None;
    let mut material_type = None;
    let mut material_name = None;
    let mut material_abbreviation = None;
    let mut brand_name = None;
    let mut nominal_weight_g = None;
    let mut actual_weight_g = None;
    let mut empty_container_weight_g = None;
    let mut nominal_length_mm = None;
    let mut actual_length_mm = None;
    let mut filament_diameter_um = None;
    let mut minimum_nozzle_diameter_um = None;
    let mut primary_color = None;
    let mut secondary_colors = Vec::new();
    let mut min_nozzle_temp = None;
    let mut max_nozzle_temp = None;
    let mut min_bed_temp = None;
    let mut max_bed_temp = None;
    let mut drying_temperature_celsius = None;
    let mut drying_time_minutes = None;

    visit_map(&mut decoder, count, |decoder, key| {
        match key {
            0 => instance_uuid = Some(decode_uuid(decoder)?),
            1 => package_uuid = Some(decode_uuid(decoder)?),
            2 => material_uuid = Some(decode_uuid(decoder)?),
            3 => brand_uuid = Some(decode_uuid(decoder)?),
            4 => gtin = Some(decoder.u64().map_err(|_| Error::InvalidCbor)?),
            5 => brand_specific_instance_id = Some(decode_string(decoder)?),
            6 => brand_specific_package_id = Some(decode_string(decoder)?),
            7 => brand_specific_material_id = Some(decode_string(decoder)?),
            8 => material_class = Some(decode_u16(decoder)?),
            9 => material_type = Some(decode_u16(decoder)?),
            10 => material_name = Some(decode_string(decoder)?),
            11 => brand_name = Some(decode_string(decoder)?),
            16 => nominal_weight_g = Some(decode_number(decoder)?),
            17 => actual_weight_g = Some(decode_number(decoder)?),
            18 => empty_container_weight_g = Some(decode_number(decoder)?),
            19 => primary_color = Some(decode_color(decoder)?),
            20..=24 => secondary_colors.push(decode_color(decoder)?),
            34 => min_nozzle_temp = Some(decode_number(decoder)?),
            35 => max_nozzle_temp = Some(decode_number(decoder)?),
            37 => min_bed_temp = Some(decode_number(decoder)?),
            38 => max_bed_temp = Some(decode_number(decoder)?),
            52 => material_abbreviation = Some(decode_string(decoder)?),
            53 => nominal_length_mm = Some(decode_number(decoder)?),
            54 => actual_length_mm = Some(decode_number(decoder)?),
            57 => drying_temperature_celsius = Some(decode_number(decoder)?),
            58 => drying_time_minutes = Some(decode_number(decoder)?),
            61 => filament_diameter_um = Some(decode_number(decoder)?),
            62 => minimum_nozzle_diameter_um = Some(decode_number(decoder)?),
            _ => decoder.skip().map_err(|_| Error::InvalidCbor)?,
        }
        Ok(())
    })?;
    if decoder.position() > MAX_SECTION_SIZE {
        return Err(Error::InvalidMainRegion);
    }

    Ok(OpenPrintTag {
        ndef_uri: None,
        instance_uuid,
        package_uuid,
        material_uuid,
        brand_uuid,
        gtin,
        brand_specific_instance_id,
        brand_specific_package_id,
        brand_specific_material_id,
        material_class: material_class.ok_or(Error::MissingMaterialClass)?,
        material_type,
        material_name,
        material_abbreviation,
        brand_name,
        nominal_weight_g,
        actual_weight_g,
        empty_container_weight_g,
        nominal_length_mm,
        actual_length_mm,
        filament_diameter_um,
        minimum_nozzle_diameter_um,
        primary_color,
        secondary_colors,
        nozzle_temperature: TemperatureRange { minimum_celsius: min_nozzle_temp, maximum_celsius: max_nozzle_temp },
        bed_temperature: TemperatureRange { minimum_celsius: min_bed_temp, maximum_celsius: max_bed_temp },
        drying_temperature_celsius,
        drying_time_minutes,
        consumed_weight_g: None,
        storage_location: None,
    })
}

fn find_ndef_uri(message: &[u8]) -> Option<String> {
    find_ndef_uri_inner(message, true)
}

fn find_ndef_uri_inner(message: &[u8], allow_smart_poster: bool) -> Option<String> {
    let mut cursor = 0;
    while cursor < message.len() {
        let header = *message.get(cursor)?;
        cursor += 1;
        if header & 0x20 != 0 {
            return None;
        }
        let short = header & 0x10 != 0;
        let has_id = header & 0x08 != 0;
        let tnf = header & 0x07;
        let type_length = *message.get(cursor)? as usize;
        cursor += 1;
        let payload_length = if short {
            let length = *message.get(cursor)? as usize;
            cursor += 1;
            length
        } else {
            let bytes = message.get(cursor..cursor + 4)?;
            cursor += 4;
            u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize
        };
        let id_length = if has_id {
            let length = *message.get(cursor)? as usize;
            cursor += 1;
            length
        } else {
            0
        };
        let record_end = cursor.checked_add(type_length)?.checked_add(id_length)?.checked_add(payload_length)?;
        if record_end > message.len() {
            return None;
        }
        let record_type = &message[cursor..cursor + type_length];
        cursor += type_length + id_length;
        let record_payload = &message[cursor..cursor + payload_length];
        if tnf == 0x01 && record_type == b"U" {
            if let Some(uri) = decode_ndef_uri(record_payload) {
                return Some(uri);
            }
        } else if allow_smart_poster && tnf == 0x01 && record_type == b"Sp" {
            if let Some(uri) = find_ndef_uri_inner(record_payload, false) {
                return Some(uri);
            }
        }
        cursor = record_end;
        if header & 0x40 != 0 {
            break;
        }
    }
    None
}

fn decode_ndef_uri(payload: &[u8]) -> Option<String> {
    let (&prefix_code, suffix) = payload.split_first()?;
    let prefix = match prefix_code {
        0x00 => "",
        0x01 => "http://www.",
        0x02 => "https://www.",
        0x03 => "http://",
        0x04 => "https://",
        0x05 => "tel:",
        0x06 => "mailto:",
        0x07 => "ftp://anonymous:anonymous@",
        0x08 => "ftp://ftp.",
        0x09 => "ftps://",
        0x0a => "sftp://",
        0x0b => "smb://",
        0x0c => "nfs://",
        0x0d => "ftp://",
        0x0e => "dav://",
        0x0f => "news:",
        0x10 => "telnet://",
        0x11 => "imap:",
        0x12 => "rtsp://",
        0x13 => "urn:",
        0x14 => "pop:",
        0x15 => "sip:",
        0x16 => "sips:",
        0x17 => "tftp:",
        0x18 => "btspp://",
        0x19 => "btl2cap://",
        0x1a => "btgoep://",
        0x1b => "tcpobex://",
        0x1c => "irdaobex://",
        0x1d => "file://",
        0x1e => "urn:epc:id:",
        0x1f => "urn:epc:tag:",
        0x20 => "urn:epc:pat:",
        0x21 => "urn:epc:raw:",
        0x22 => "urn:epc:",
        0x23 => "urn:nfc:",
        _ => return None,
    };
    let suffix = core::str::from_utf8(suffix).ok()?;
    let mut uri = String::from(prefix);
    uri.push_str(suffix);
    Some(uri)
}

fn decode_aux(data: &[u8], tag: &mut OpenPrintTag) -> Result<(), Error> {
    let mut decoder = Decoder::new(data);
    let count = decoder.map().map_err(|_| Error::InvalidCbor)?;
    visit_map(&mut decoder, count, |decoder, key| {
        match key {
            0 => tag.consumed_weight_g = Some(decode_number(decoder)?),
            4 => tag.storage_location = Some(decode_string(decoder)?),
            _ => decoder.skip().map_err(|_| Error::InvalidCbor)?,
        }
        Ok(())
    })?;
    if decoder.position() > MAX_SECTION_SIZE {
        return Err(Error::InvalidAuxRegion);
    }
    Ok(())
}

fn visit_map<F>(decoder: &mut Decoder<'_>, count: Option<u64>, mut visit: F) -> Result<(), Error>
where
    F: FnMut(&mut Decoder<'_>, u64) -> Result<(), Error>,
{
    match count {
        Some(count) => {
            for _ in 0..count {
                let key = decoder.u64().map_err(|_| Error::InvalidCbor)?;
                visit(decoder, key)?;
            }
        }
        None => loop {
            if decoder.datatype().map_err(|_| Error::InvalidCbor)? == Type::Break {
                decoder.skip().map_err(|_| Error::InvalidCbor)?;
                break;
            }
            let key = decoder.u64().map_err(|_| Error::InvalidCbor)?;
            visit(decoder, key)?;
        },
    }
    Ok(())
}

fn decode_uuid(decoder: &mut Decoder<'_>) -> Result<[u8; 16], Error> {
    let bytes = decoder.bytes().map_err(|_| Error::InvalidCbor)?;
    bytes.try_into().map_err(|_| Error::InvalidCbor)
}

fn decode_color(decoder: &mut Decoder<'_>) -> Result<Color, Error> {
    let bytes = decoder.bytes().map_err(|_| Error::InvalidCbor)?;
    match bytes {
        [red, green, blue] => Ok(Color { red: *red, green: *green, blue: *blue, alpha: 255 }),
        [red, green, blue, alpha] => Ok(Color { red: *red, green: *green, blue: *blue, alpha: *alpha }),
        _ => Err(Error::InvalidCbor),
    }
}

fn decode_string(decoder: &mut Decoder<'_>) -> Result<String, Error> {
    decoder.str().map(String::from).map_err(|_| Error::InvalidCbor)
}

fn decode_u16(decoder: &mut Decoder<'_>) -> Result<u16, Error> {
    decoder.u16().map_err(|_| Error::InvalidCbor)
}

fn decode_usize(decoder: &mut Decoder<'_>) -> Result<usize, Error> {
    decoder.u64().map(|value| value as usize).map_err(|_| Error::InvalidCbor)
}

fn decode_number(decoder: &mut Decoder<'_>) -> Result<f64, Error> {
    match decoder.datatype().map_err(|_| Error::InvalidCbor)? {
        Type::U8 => decoder.u8().map(|value| value as f64),
        Type::U16 => decoder.u16().map(|value| value as f64),
        Type::U32 => decoder.u32().map(|value| value as f64),
        Type::U64 => decoder.u64().map(|value| value as f64),
        Type::I8 => decoder.i8().map(|value| value as f64),
        Type::I16 => decoder.i16().map(|value| value as f64),
        Type::I32 => decoder.i32().map(|value| value as f64),
        Type::I64 => decoder.i64().map(|value| value as f64),
        Type::F16 => decoder.f16().map(|value| value as f64),
        Type::F32 => decoder.f32().map(|value| value as f64),
        Type::F64 => decoder.f64(),
        _ => return Err(Error::InvalidCbor),
    }
    .map_err(|_| Error::InvalidCbor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use minicbor::Encoder;

    #[test]
    fn decodes_payload_with_indefinite_unsorted_maps_and_unknown_fields() {
        let mut main = Vec::new();
        Encoder::new(&mut main)
            .begin_map().unwrap()
            .u8(99).unwrap().begin_array().unwrap().u8(1).unwrap().end().unwrap()
            .u8(19).unwrap().bytes(&[0x12, 0x34, 0x56]).unwrap()
            .u8(11).unwrap().str("Prusa").unwrap()
            .u8(8).unwrap().u8(0).unwrap()
            .u8(10).unwrap().str("PLA Galaxy Black").unwrap()
            .u8(16).unwrap().u16(1000).unwrap()
            .u8(61).unwrap().u16(1750).unwrap()
            .end().unwrap();
        let mut payload = vec![0xa2, 0x01, 0x18, 0x40, 0x00, 0x06];
        payload.extend_from_slice(&main);
        payload.resize(70, 0);

        let tag = decode_payload(&payload).unwrap();
        assert_eq!(tag.material_class, 0);
        assert_eq!(tag.brand_name.as_deref(), Some("Prusa"));
        assert_eq!(tag.material_name.as_deref(), Some("PLA Galaxy Black"));
        assert_eq!(tag.nominal_weight_g, Some(1000.0));
        assert_eq!(tag.filament_diameter_um, Some(1750.0));
        assert_eq!(tag.primary_color, Some(Color { red: 0x12, green: 0x34, blue: 0x56, alpha: 255 }));
    }

    #[test]
    fn finds_openprinttag_after_another_ndef_record() {
        let payload = [0xa0, 0xa1, 0x08, 0x00];
        let uri_suffix = b"www.prusa3d.com/product/test/";
        let mut ndef = vec![0x91, 0x01, (uri_suffix.len() + 1) as u8, b'U', 0x04];
        ndef.extend_from_slice(uri_suffix);
        ndef.extend_from_slice(&[0x52, MIME_TYPE.len() as u8, payload.len() as u8]);
        ndef.extend_from_slice(MIME_TYPE);
        ndef.extend_from_slice(&payload);
        let mut memory = vec![0xe1, 0x40, 0x28, 0x01, 0x03, ndef.len() as u8];
        memory.extend_from_slice(&ndef);
        memory.push(0xfe);

        let tag = decode_tag_memory(&memory).unwrap();
        assert_eq!(tag.material_class, 0);
        assert_eq!(tag.ndef_uri.as_deref(), Some("https://www.prusa3d.com/product/test/"));
    }

    #[test]
    fn decodes_official_openprinttag_sample() {
        // openprinttag-specification revision 7e09cc3, docs_src/sample_data/sample_tag.bin
        let memory = decode_hex(concat!(
            "e140260103ff0127c21c000001056170706c69636174696f6e2f766e642e",
            "6f70656e7072696e74746167a10218e2bf0050473bb8cde12945b89fcfda",
            "1c3add9c47076131080009000a70504c412047616c61787920426c61636b",
            "0b6950727573616d656e740e1a67acb31a101903e8111903f41218641343",
            "3d3e3d181bf93266181c9f17ff182218cd182318dc182418aa1825182818",
            "26183c18281828182914182a184bff000000000000000000000000000000",
            "000000000000000000000000000000000000000000000000000000000000",
            "000000000000000000000000000000000000000000000000000000000000",
            "00000000000000000000000000000000000000000000000000000000a000",
            "000000000000000000000000000000000000000000000000000000000000",
            "000000fe"
        ));
        let tag = decode_tag_memory(&memory).unwrap();
        assert_eq!(tag.material_class, 0);
        assert_eq!(tag.material_type, Some(0));
        assert_eq!(tag.brand_name.as_deref(), Some("Prusament"));
        assert_eq!(tag.material_name.as_deref(), Some("PLA Galaxy Black"));
        assert_eq!(tag.nominal_weight_g, Some(1000.0));
        assert_eq!(tag.actual_weight_g, Some(1012.0));
        assert_eq!(tag.primary_color, Some(Color { red: 0x3d, green: 0x3e, blue: 0x3d, alpha: 255 }));
    }

    #[test]
    fn derives_stable_rfc4122_instance_uuid_from_canonical_uid() {
        let uuid = derive_instance_uuid(&[0xe0, 0x04, 0x01, 0x08, 0x66, 0x2f, 0x6f, 0xbc]);
        assert_eq!(uuid[6] >> 4, 5);
        assert_eq!(uuid[8] >> 6, 2);
        assert_eq!(uuid, derive_instance_uuid(&[0xe0, 0x04, 0x01, 0x08, 0x66, 0x2f, 0x6f, 0xbc]));
    }

    #[test]
    fn supports_auxiliary_region_before_main_region() {
        let mut payload = vec![0xa4, 0x00, 0x18, 0x20, 0x01, 0x08, 0x02, 0x10, 0x03, 0x08];
        payload.resize(16, 0);
        payload.extend_from_slice(&[0xa1, 0x00, 0x18, 0x64]);
        payload.resize(32, 0);
        payload.extend_from_slice(&[0xa1, 0x08, 0x00]);
        payload.resize(40, 0);

        let tag = decode_payload(&payload).unwrap();
        assert_eq!(tag.material_class, 0);
        assert_eq!(tag.consumed_weight_g, Some(100.0));
    }

    fn decode_hex(input: &str) -> Vec<u8> {
        input
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| (hex_nibble(pair[0]) << 4) | hex_nibble(pair[1]))
            .collect()
    }

    fn hex_nibble(value: u8) -> u8 {
        match value {
            b'0'..=b'9' => value - b'0',
            b'a'..=b'f' => value - b'a' + 10,
            _ => panic!("invalid test hex"),
        }
    }
}
