use alloc::{format, string::String, vec, vec::Vec};

use formats::openprinttag::{self, OpenPrintTag};

use crate::bambu_spool::BambuSpool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpoolSource {
    BambuLab,
    OpenPrintTag,
}

#[derive(Debug, Clone)]
pub enum ProductReference {
    Bambu { color_code: String },
    OpenPrintTag {
        brand_uuid: Option<[u8; 16]>,
        material_uuid: Option<[u8; 16]>,
        gtin: Option<u64>,
        brand_name: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub struct FilamentSpool {
    pub source: SpoolSource,
    pub external_id: String,
    pub tag_uid: String,
    pub brand: Option<String>,
    pub material_name: String,
    pub material_type: String,
    pub source_detail: String,
    pub color_name: String,
    pub colors: Vec<[u8; 4]>,
    pub nominal_weight_g: Option<f64>,
    pub remaining_weight_g: Option<f64>,
    pub empty_container_weight_g: Option<f64>,
    pub diameter_mm: Option<f64>,
    pub length_m: Option<f64>,
    pub nozzle_min_c: Option<f64>,
    pub nozzle_max_c: Option<f64>,
    pub bed_min_c: Option<f64>,
    pub bed_max_c: Option<f64>,
    pub drying_temperature_c: Option<f64>,
    pub drying_time_h: Option<f64>,
    pub additional_details: Vec<String>,
    pub product_reference: ProductReference,
}

impl FilamentSpool {
    pub fn from_bambu(spool: &BambuSpool) -> Self {
        let mut colors = vec![spool.primary_rgba];
        if let Some(secondary) = spool.secondary_rgba {
            colors.push(secondary);
        }
        Self {
            source: SpoolSource::BambuLab,
            external_id: spool.tray_uid.clone(),
            tag_uid: spool.tag_uid.clone(),
            brand: Some(String::from("Bambu Lab")),
            material_name: spool.official_material_name.clone(),
            material_type: spool.filament_type.clone(),
            source_detail: format!("{} · {}", spool.material_id, spool.variant_id),
            color_name: spool.display_color_name.clone(),
            colors,
            nominal_weight_g: Some(spool.weight_g as f64),
            remaining_weight_g: None,
            empty_container_weight_g: None,
            diameter_mm: Some(spool.diameter_mm as f64),
            length_m: Some(spool.filament_length_m as f64),
            nozzle_min_c: Some(spool.nozzle_temperature_min_c as f64),
            nozzle_max_c: Some(spool.nozzle_temperature_max_c as f64),
            bed_min_c: Some(spool.bed_temperature_c as f64),
            bed_max_c: Some(spool.bed_temperature_c as f64),
            drying_temperature_c: Some(spool.drying_temperature_c as f64),
            drying_time_h: Some(spool.drying_time_h as f64),
            additional_details: vec![
                format!("Produced {}", spool.production_date),
                format!("Tag type {}", spool.detailed_filament_type),
                format!("Spool width {:.2} mm", spool.spool_width_mm),
            ],
            product_reference: ProductReference::Bambu { color_code: spool.bambu_color_code.clone() },
        }
    }

    pub fn from_openprinttag(tag_uid: &[u8], tag: &OpenPrintTag) -> Self {
        let material_type = tag
            .material_abbreviation
            .clone()
            .or_else(|| tag.material_type.and_then(openprinttag::material_type_abbreviation).map(String::from))
            .unwrap_or_else(|| String::from("FFF"));
        let material_name = tag.material_name.clone().unwrap_or_else(|| material_type.clone());
        let colors: Vec<[u8; 4]> = tag
            .primary_color
            .iter()
            .chain(tag.secondary_colors.iter())
            .map(|color| [color.red, color.green, color.blue, color.alpha])
            .collect();
        let full_weight = tag.actual_weight_g.or(tag.nominal_weight_g);
        let remaining_weight_g = full_weight.map(|weight| (weight - tag.consumed_weight_g.unwrap_or(0.0)).max(0.0));
        let external_id = tag
            .instance_uuid
            .or_else(|| <[u8; 8]>::try_from(tag_uid).ok().map(|uid| openprinttag::derive_instance_uuid(&uid)))
            .map(format_uuid)
            .unwrap_or_else(|| hex::encode_upper(tag_uid));
        let mut additional_details = Vec::new();
        if let Some(location) = &tag.storage_location {
            additional_details.push(format!("Tag location {location}"));
        }
        let color_name = product_variant_name(&material_name, &material_type);
        Self {
            source: SpoolSource::OpenPrintTag,
            external_id,
            tag_uid: hex::encode_upper(tag_uid),
            brand: tag.brand_name.clone(),
            material_name,
            material_type,
            source_detail: String::from("OpenPrintTag"),
            color_name,
            colors,
            nominal_weight_g: full_weight,
            remaining_weight_g,
            empty_container_weight_g: tag.empty_container_weight_g,
            diameter_mm: tag.filament_diameter_um.map(|value| value / 1000.0),
            length_m: tag.actual_length_mm.or(tag.nominal_length_mm).map(|value| value / 1000.0),
            nozzle_min_c: tag.nozzle_temperature.minimum_celsius,
            nozzle_max_c: tag.nozzle_temperature.maximum_celsius,
            bed_min_c: tag.bed_temperature.minimum_celsius,
            bed_max_c: tag.bed_temperature.maximum_celsius,
            drying_temperature_c: tag.drying_temperature_celsius,
            drying_time_h: tag.drying_time_minutes.map(|value| value / 60.0),
            additional_details,
            product_reference: ProductReference::OpenPrintTag {
                brand_uuid: tag.brand_uuid,
                material_uuid: tag.material_uuid,
                gtin: tag.gtin,
                brand_name: tag.brand_name.clone(),
            },
        }
    }

    pub fn display_name(&self) -> String {
        if self.source == SpoolSource::BambuLab {
            return self.material_name.clone();
        }
        match self.brand.as_deref() {
            Some(brand) if !self.material_name.starts_with(brand) => format!("{brand} {}", self.material_name),
            _ => self.material_name.clone(),
        }
    }

    pub fn primary_color(&self) -> [u8; 4] {
        self.colors.first().copied().unwrap_or([0x33, 0x3b, 0x45, 0xff])
    }
}

fn format_uuid(uuid: [u8; 16]) -> String {
    let encoded = hex::encode(uuid);
    format!(
        "{}-{}-{}-{}-{}",
        &encoded[0..8],
        &encoded[8..12],
        &encoded[12..16],
        &encoded[16..20],
        &encoded[20..32]
    )
}

fn product_variant_name(material_name: &str, material_type: &str) -> String {
    material_name
        .strip_prefix(material_type)
        .map(|suffix| suffix.trim_start_matches([' ', '-', '·', '/']).trim())
        .filter(|suffix| !suffix.is_empty())
        .map(String::from)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::product_variant_name;

    #[test]
    fn uses_openprinttag_material_suffix_as_product_variant() {
        assert_eq!(product_variant_name("PLA Galaxy Black", "PLA"), "Galaxy Black");
        assert_eq!(product_variant_name("PETG", "PETG"), "");
    }
}
