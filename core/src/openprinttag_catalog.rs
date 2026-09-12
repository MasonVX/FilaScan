use alloc::{
    format,
    rc::Rc,
    string::{String, ToString},
    vec::Vec,
};
use core::cell::RefCell;

use edge_http::Method;
use framework::framework::Framework;
use serde::Deserialize;

use crate::{
    image_loader::{self, GTS_ROOT_R4, LoadedProductImage},
    spool::FilamentSpool,
};

const DATABASE_HOST: &str = "database.openprinttag.org";
const MAX_MATERIAL_BYTES: usize = 24 * 1024;

#[derive(Debug, Clone, Copy)]
pub enum CatalogSource {
    SdCard,
    OpenPrintTagDatabase,
}

impl CatalogSource {
    pub const fn label(self) -> &'static str {
        match self {
            Self::SdCard => "SD cache",
            Self::OpenPrintTagDatabase => "OpenPrintTag database",
        }
    }
}

pub struct CatalogEnrichment {
    pub material: CatalogMaterial,
    pub source: CatalogSource,
    pub image: Option<LoadedProductImage>,
    pub image_error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CatalogMaterial {
    uuid: String,
    slug: String,
    brand: CatalogReference,
    name: String,
    #[serde(rename = "type")]
    material_type: Option<String>,
    primary_color: Option<CatalogColor>,
    #[serde(default)]
    photos: Vec<CatalogPhoto>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    certifications: Vec<String>,
    transmission_distance: Option<f64>,
    #[serde(default)]
    properties: CatalogProperties,
}

#[derive(Debug, Clone, Deserialize)]
struct CatalogReference {
    slug: String,
}

#[derive(Debug, Clone, Deserialize)]
struct CatalogColor {
    color_rgba: String,
}

#[derive(Debug, Clone, Deserialize)]
struct CatalogPhoto {
    url: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct CatalogProperties {
    density: Option<f64>,
    hardness_shore_a: Option<f64>,
    hardness_shore_d: Option<f64>,
    min_print_temperature: Option<f64>,
    max_print_temperature: Option<f64>,
    min_bed_temperature: Option<f64>,
    max_bed_temperature: Option<f64>,
    drying_temperature: Option<f64>,
    drying_time: Option<f64>,
}

impl CatalogMaterial {
    pub fn apply_to(&self, spool: &mut FilamentSpool) {
        if spool.material_name.is_empty() {
            spool.material_name = self.name.clone();
        }
        if spool.material_type == "FFF" {
            if let Some(material_type) = &self.material_type {
                spool.material_type = material_type.clone();
            }
        }
        if spool.colors.is_empty() {
            if let Some(color) = self.primary_color.as_ref().and_then(|color| parse_rgba(&color.color_rgba)) {
                spool.colors.push(color);
            }
        }
        spool.nozzle_min_c = spool.nozzle_min_c.or(self.properties.min_print_temperature);
        spool.nozzle_max_c = spool.nozzle_max_c.or(self.properties.max_print_temperature);
        spool.bed_min_c = spool.bed_min_c.or(self.properties.min_bed_temperature);
        spool.bed_max_c = spool.bed_max_c.or(self.properties.max_bed_temperature);
        spool.drying_temperature_c = spool.drying_temperature_c.or(self.properties.drying_temperature);
        spool.drying_time_h = spool.drying_time_h.or(self.properties.drying_time.map(|minutes| minutes / 60.0));

        push_detail(&mut spool.additional_details, "OpenPrintTag DB");
        if !self.tags.is_empty() {
            push_detail(&mut spool.additional_details, &self.tags.join(", "));
        }
        if !self.certifications.is_empty() {
            let certifications = self
                .certifications
                .iter()
                .map(|value| value.replace('_', " ").to_uppercase())
                .collect::<Vec<_>>()
                .join(", ");
            push_detail(&mut spool.additional_details, &certifications);
        }
        if let Some(density) = self.properties.density {
            push_detail(&mut spool.additional_details, &format!("ρ {density:.2} g/cm³"));
        }
        if let Some(hardness) = self.properties.hardness_shore_a {
            push_detail(&mut spool.additional_details, &format!("Shore A {hardness:.0}"));
        }
        if let Some(hardness) = self.properties.hardness_shore_d {
            push_detail(&mut spool.additional_details, &format!("Shore D {hardness:.0}"));
        }
        if let Some(distance) = self.transmission_distance {
            push_detail(&mut spool.additional_details, &format!("TD {distance:.1}"));
        }
    }

    pub fn log_summary(&self) -> String {
        format!(
            "{} (UUID {}, color {}, {} photo(s))",
            self.name,
            self.uuid,
            self.primary_color
                .as_ref()
                .map(|color| color.color_rgba.as_str())
                .unwrap_or("not provided"),
            self.photos.len()
        )
    }
}

pub async fn load(framework: Rc<RefCell<Framework>>, spool: &FilamentSpool) -> Result<CatalogEnrichment, String> {
    let (brand_slug, material_slug) = lookup_slugs(spool)?;
    let cache_key = cache_key(&material_slug);
    let cache_path = format!("/filascan/optag/{cache_key}.jsn");
    let file_store = framework.borrow().file_store();
    let sdcard_available = file_store.lock().await.card_installed;

    let mut cached_material = None;
    if sdcard_available {
        if let Ok(bytes) = file_store.lock().await.read_file_bytes(&cache_path).await {
            if let Ok(material) = parse_material(&bytes, &brand_slug, &material_slug) {
                cached_material = Some(material);
            }
        }
    }

    let (material, source) = if let Some(material) = cached_material {
        (material, CatalogSource::SdCard)
    } else {
        if framework.borrow().wifi_ok != Some(true) {
            return Err("catalog entry is not cached and Wi-Fi is not connected".to_string());
        }
        let request_path = format!("/api/brands/{brand_slug}/materials/{material_slug}.json");
        let (stack, tls) = {
            let framework = framework.borrow();
            (framework.stack, framework.tls)
        };
        let bytes = image_loader::https_request(
            stack,
            tls,
            DATABASE_HOST,
            &request_path,
            Method::Get,
            &[
                ("Host", DATABASE_HOST),
                ("Accept", "application/json"),
                ("User-Agent", "FilaScan/0.2 (OpenPrintTag)"),
                ("Connection", "close"),
            ],
            None,
            GTS_ROOT_R4,
            MAX_MATERIAL_BYTES,
        )
        .await?;
        let material = parse_material(&bytes, &brand_slug, &material_slug)?;
        if sdcard_available {
            let _ = file_store.lock().await.create_write_file_bytes(&cache_path, &bytes).await;
        }
        (material, CatalogSource::OpenPrintTagDatabase)
    };

    let supported_photo = material
        .photos
        .iter()
        .find(|photo| photo.url.starts_with("https://files.openprinttag.org/"));
    let (image, image_error) = match supported_photo {
        Some(photo) => match image_loader::load_openprinttag_image(framework, &cache_key, &photo.url).await {
            Ok(image) => (Some(image), None),
            Err(error) => (None, Some(error)),
        },
        None => (None, Some("catalog entry has no product photo".to_string())),
    };
    Ok(CatalogEnrichment {
        material,
        source,
        image,
        image_error,
    })
}

fn lookup_slugs(spool: &FilamentSpool) -> Result<(String, String), String> {
    let brand = spool.brand.as_deref().ok_or_else(|| "OpenPrintTag has no brand name".to_string())?;
    let brand_slug = slugify(brand);
    let name_slug = slugify(&spool.material_name);
    if brand_slug.is_empty() || name_slug.is_empty() {
        return Err("OpenPrintTag brand or material name cannot be mapped to a database slug".to_string());
    }
    let material_slug = if name_slug == brand_slug || name_slug.starts_with(&format!("{brand_slug}-")) {
        name_slug
    } else {
        format!("{brand_slug}-{name_slug}")
    };
    Ok((brand_slug, material_slug))
}

fn slugify(value: &str) -> String {
    let mut result = String::new();
    let mut separator = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !result.is_empty() {
                result.push('-');
            }
            result.push(character.to_ascii_lowercase());
            separator = false;
        } else if !result.is_empty() {
            separator = true;
        }
    }
    result
}

fn cache_key(value: &str) -> String {
    let mut hash = 0x811c9dc5_u32;
    for byte in value.bytes() {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(0x01000193);
    }
    format!("{hash:08X}")
}

fn parse_material(bytes: &[u8], brand_slug: &str, material_slug: &str) -> Result<CatalogMaterial, String> {
    let material: CatalogMaterial = serde_json::from_slice(bytes).map_err(|error| format!("invalid OpenPrintTag catalog JSON: {error}"))?;
    if material.brand.slug != brand_slug || material.slug != material_slug {
        return Err("OpenPrintTag catalog entry does not match the requested material".to_string());
    }
    Ok(material)
}

fn parse_rgba(value: &str) -> Option<[u8; 4]> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 && hex.len() != 8 {
        return None;
    }
    let mut color = [0_u8; 4];
    for (index, component) in color.iter_mut().take(3).enumerate() {
        *component = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).ok()?;
    }
    color[3] = if hex.len() == 8 {
        u8::from_str_radix(&hex[6..8], 16).ok()?
    } else {
        0xff
    };
    Some(color)
}

fn push_detail(details: &mut Vec<String>, detail: &str) {
    if !detail.is_empty() && !details.iter().any(|existing| existing == detail) {
        details.push(detail.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::{cache_key, parse_rgba, slugify};

    #[test]
    fn builds_database_compatible_slugs() {
        assert_eq!(slugify("Prusament"), "prusament");
        assert_eq!(slugify("PETG Prusa Galaxy Black"), "petg-prusa-galaxy-black");
    }

    #[test]
    fn cache_keys_are_short_and_stable() {
        assert_eq!(cache_key("prusament-petg-prusa-galaxy-black").len(), 8);
        assert_eq!(
            cache_key("prusament-petg-prusa-galaxy-black"),
            cache_key("prusament-petg-prusa-galaxy-black")
        );
    }

    #[test]
    fn parses_catalog_rgba_colors() {
        assert_eq!(parse_rgba("#3d3e3dff"), Some([0x3d, 0x3e, 0x3d, 0xff]));
        assert_eq!(parse_rgba("#ff0000"), Some([0xff, 0, 0, 0xff]));
    }
}
