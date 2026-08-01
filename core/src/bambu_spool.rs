use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use hashbrown::HashMap;

const MATERIAL_NAMES: &str = include_str!("../data/base-filaments-index.csv");
const COLOR_NAMES: &str = include_str!("../data/bambu-color-names.csv");

#[derive(Debug, Clone)]
pub struct BambuSpool {
    pub tag_uid: String,
    pub spool_uid: String,
    pub material_id: String,
    pub variant_id: String,
    pub filament_type: String,
    pub detailed_filament_type: String,
    pub official_material_name: String,
    pub primary_rgba: [u8; 4],
    pub secondary_rgba: Option<[u8; 4]>,
    pub color_hex: String,
    pub color_name: String,
    pub bambu_color_code: String,
    pub weight_g: u16,
    pub diameter_mm: f64,
    pub drying_temperature_c: u16,
    pub drying_time_h: u16,
    pub bed_temperature_c: u16,
    pub nozzle_temperature_min_c: u16,
    pub nozzle_temperature_max_c: u16,
    pub spool_width_mm: f32,
    pub filament_length_m: u16,
    pub production_date: String,
}

impl BambuSpool {
    pub fn from_tag(tag_uid: &[u8], blocks: &HashMap<i32, Vec<u8>>) -> Self {
        let material_id = block_string(blocks, 1, 8, 8);
        let variant_id = block_string(blocks, 1, 0, 8);
        let filament_type = block_string(blocks, 2, 0, 16);
        let detailed_filament_type = block_string(blocks, 4, 0, 16);
        let primary_rgba = block_array::<4>(blocks, 5, 0).unwrap_or([0x33, 0x3b, 0x45, 0xff]);

        let secondary_rgba = block_array::<8>(blocks, 16, 0).and_then(|data| {
            let format_identifier = u16::from_le_bytes([data[0], data[1]]);
            let color_count = u16::from_le_bytes([data[2], data[3]]);
            (format_identifier == 2 && color_count > 1).then_some([data[7], data[6], data[5], data[4]])
        });

        let primary_hex = hex::encode_upper(primary_rgba);
        let secondary_hex = secondary_rgba.map(hex::encode_upper);
        let combined_colors = match &secondary_hex {
            Some(second) => format!("{primary_hex}/{second}"),
            None => primary_hex.clone(),
        };

        let (color_name, bambu_color_code) =
            lookup_color(&material_id, &combined_colors).unwrap_or_else(|| ("Unknown Bambu color".to_string(), String::new()));

        let official_material_name = lookup_material(&material_id).unwrap_or_else(|| detailed_filament_type.clone());

        let production_date = {
            let full = block_string(blocks, 12, 0, 16);
            if full.is_empty() { block_string(blocks, 13, 0, 16) } else { full }
        };

        Self {
            tag_uid: hex::encode_upper(tag_uid),
            spool_uid: block_string(blocks, 9, 0, 16),
            material_id,
            variant_id,
            filament_type,
            detailed_filament_type,
            official_material_name,
            primary_rgba,
            secondary_rgba,
            color_hex: combined_colors,
            color_name,
            bambu_color_code,
            weight_g: block_u16(blocks, 5, 4),
            diameter_mm: block_f64(blocks, 5, 8),
            drying_temperature_c: block_u16(blocks, 6, 0),
            drying_time_h: block_u16(blocks, 6, 2),
            bed_temperature_c: block_u16(blocks, 6, 6),
            nozzle_temperature_max_c: block_u16(blocks, 6, 8),
            nozzle_temperature_min_c: block_u16(blocks, 6, 10),
            spool_width_mm: block_u16(blocks, 10, 4) as f32 / 100.0,
            filament_length_m: block_u16(blocks, 14, 4),
            production_date,
        }
    }
}

fn lookup_material(material_id: &str) -> Option<String> {
    MATERIAL_NAMES.lines().find_map(|line| {
        let mut columns = line.split(',');
        (columns.next()? == material_id).then(|| columns.next().unwrap_or_default().to_string())
    })
}

fn lookup_color(material_id: &str, colors: &str) -> Option<(String, String)> {
    COLOR_NAMES.lines().find_map(|line| {
        let mut columns = line.split(',');
        let row_material = columns.next()?;
        let row_colors = columns.next()?;
        let name = columns.next()?;
        let code = columns.next().unwrap_or_default();
        (row_material == material_id && row_colors == colors).then(|| (name.to_string(), code.to_string()))
    })
}

fn block_string(blocks: &HashMap<i32, Vec<u8>>, block: i32, start: usize, len: usize) -> String {
    blocks
        .get(&block)
        .and_then(|bytes| bytes.get(start..start + len))
        .and_then(|bytes| {
            let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(bytes.len());
            core::str::from_utf8(&bytes[..end]).ok()
        })
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn block_array<const N: usize>(blocks: &HashMap<i32, Vec<u8>>, block: i32, start: usize) -> Option<[u8; N]> {
    blocks.get(&block)?.get(start..start + N)?.try_into().ok()
}

fn block_u16(blocks: &HashMap<i32, Vec<u8>>, block: i32, start: usize) -> u16 {
    block_array::<2>(blocks, block, start).map(u16::from_le_bytes).unwrap_or_default()
}

fn block_f64(blocks: &HashMap<i32, Vec<u8>>, block: i32, start: usize) -> f64 {
    block_array::<8>(blocks, block, start)
        .map(f64::from_le_bytes)
        .filter(|value| value.is_finite())
        .unwrap_or_default()
}
