use alloc::{
    boxed::Box,
    ffi::CString,
    format,
    rc::Rc,
    string::{String, ToString},
    vec::Vec,
};
use core::{
    cell::RefCell,
    net::{IpAddr, SocketAddr},
};

use edge_http::{Method, io::client::Connection};
use edge_nal_embassy::{Tcp, TcpBuffers};
use embassy_net::{IpAddress, Stack};
use embedded_io_async::{Read, Write};
use esp_mbedtls::{Certificates, TlsReference, TlsVersion, X509};
use framework::framework::Framework;
use serde_json::{Value, json};
use slint::{Image, Rgb8Pixel, SharedPixelBuffer};
use zune_core::{colorspace::ColorSpace, options::DecoderOptions};
use zune_jpeg::JpegDecoder;

const AMAZON_ROOT_CA_1: &str = include_str!("certs/amazon-root-ca-1.pem");
const DIGICERT_GLOBAL_ROOT_G2: &str = include_str!("certs/digicert-global-root-g2.pem");
pub(crate) const GTS_ROOT_R4: &str = include_str!("certs/gts-root-r4.pem");
const STORE_API_HOST: &str = "eu-store-api.bambulab.com";
const STORE_API_PATH: &str = "/mall-goods/product/globalSearchV2";
const STORE_REGION: &str = "EU";
const MAX_SEARCH_BYTES: usize = 128 * 1024;
const MAX_IMAGE_BYTES: usize = 96 * 1024;
const MAX_IMAGE_DIMENSION: u16 = 240;

#[derive(Debug, Clone, Copy)]
pub enum ProductImageSource {
    SdCard,
    BambuCdn,
    OpenPrintTagCdn,
}

pub async fn load_openprinttag_image(
    framework: Rc<RefCell<Framework>>,
    cache_key: &str,
    image_url: &str,
) -> Result<LoadedProductImage, String> {
    if cache_key.len() != 8 || !cache_key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("OpenPrintTag image cache key is invalid".to_string());
    }
    let cache_path = format!("/filascan/optag/{cache_key}.jpg");
    let file_store = framework.borrow().file_store();
    let sdcard_available = file_store.lock().await.card_installed;
    if sdcard_available {
        if let Ok(encoded) = file_store.lock().await.read_file_bytes(&cache_path).await {
            if encoded.len() <= MAX_IMAGE_BYTES {
                if let Ok(image) = decode_jpeg(&encoded) {
                    return Ok(LoadedProductImage {
                        image,
                        source: ProductImageSource::SdCard,
                    });
                }
            }
        }
    }

    if framework.borrow().wifi_ok != Some(true) {
        return Err("not cached and Wi-Fi is not connected".to_string());
    }
    let (host, source_path) = parse_openprinttag_image_url(image_url)?;
    let optimized_path = format!("/cdn-cgi/image/width=240,format=jpg,quality=80{source_path}");
    let (stack, tls) = {
        let framework = framework.borrow();
        (framework.stack, framework.tls)
    };
    let encoded = https_request(
        stack,
        tls,
        host,
        &optimized_path,
        Method::Get,
        &[
            ("Host", host),
            ("Accept", "image/jpeg"),
            ("User-Agent", "FilaScan/0.2 (OpenPrintTag)"),
            ("Connection", "close"),
        ],
        None,
        GTS_ROOT_R4,
        MAX_IMAGE_BYTES,
    )
    .await?;
    let image = decode_jpeg(&encoded)?;
    if sdcard_available {
        let _ = file_store.lock().await.create_write_file_bytes(&cache_path, &encoded).await;
    }
    Ok(LoadedProductImage {
        image,
        source: ProductImageSource::OpenPrintTagCdn,
    })
}

pub struct LoadedProductImage {
    pub image: Image,
    pub source: ProductImageSource,
}

pub async fn load_product_image(framework: Rc<RefCell<Framework>>, product_code: &str) -> Result<LoadedProductImage, String> {
    validate_product_code(product_code)?;
    let cache_path = cache_path(product_code);
    let file_store = framework.borrow().file_store();
    let sdcard_available = file_store.lock().await.card_installed;
    if sdcard_available {
        if let Ok(encoded) = file_store.lock().await.read_file_bytes(&cache_path).await {
            if encoded.len() <= MAX_IMAGE_BYTES {
                if let Ok(image) = decode_jpeg(&encoded) {
                    return Ok(LoadedProductImage {
                        image,
                        source: ProductImageSource::SdCard,
                    });
                }
            }
        }
    }

    if framework.borrow().wifi_ok != Some(true) {
        return Err("not cached and Wi-Fi is not connected".to_string());
    }

    let (stack, tls) = {
        let framework = framework.borrow();
        (framework.stack, framework.tls)
    };
    let image_url = resolve_store_image(stack, tls, product_code).await?;
    let encoded = download_product_image(stack, tls, &image_url).await?;
    let image = decode_jpeg(&encoded)?;
    if sdcard_available {
        let _ = file_store.lock().await.create_write_file_bytes(&cache_path, &encoded).await;
    }
    Ok(LoadedProductImage {
        image,
        source: ProductImageSource::BambuCdn,
    })
}

async fn resolve_store_image(stack: Stack<'static>, tls: TlsReference<'static>, product_code: &str) -> Result<String, String> {
    let body = serde_json::to_vec(&json!({
        "content": product_code,
        "current": 1,
        "size": 4
    }))
    .map_err(|error| format!("Store search serialization failed: {error}"))?;
    let content_length = body.len().to_string();
    let headers = [
        ("Host", STORE_API_HOST),
        ("Accept", "application/json"),
        ("Content-Type", "application/json"),
        ("Content-Length", content_length.as_str()),
        ("X-BBL-STORE-REGION", STORE_REGION),
        ("User-Agent", "FilaScan/0.1 (store-search-image)"),
        ("Connection", "close"),
    ];
    let response = https_request(
        stack,
        tls,
        STORE_API_HOST,
        STORE_API_PATH,
        Method::Post,
        &headers,
        Some(&body),
        DIGICERT_GLOBAL_ROOT_G2,
        MAX_SEARCH_BYTES,
    )
    .await?;
    let payload: Value = serde_json::from_slice(&response).map_err(|error| format!("Invalid Bambu store search JSON: {error}"))?;
    parse_store_search_image(&payload)
}

fn parse_store_search_image(payload: &Value) -> Result<String, String> {
    if payload.get("code").and_then(Value::as_u64) != Some(1) {
        return Err("Bambu store search did not succeed".to_string());
    }
    let page = payload
        .get("data")
        .and_then(|value| value.get("page"))
        .ok_or_else(|| "Bambu store search has no result page".to_string())?;
    let total_is_one = page
        .get("total")
        .map(|value| value.as_str() == Some("1") || value.as_u64() == Some(1))
        .unwrap_or(false);
    let records = page
        .get("records")
        .and_then(Value::as_array)
        .ok_or_else(|| "Bambu store search has no records".to_string())?;
    if !total_is_one || records.len() != 1 {
        return Err("Bambu store search did not return exactly one product".to_string());
    }
    let record = &records[0];
    let sku_id = record.get("highlightProductSkuId").and_then(Value::as_str).unwrap_or_default();
    if sku_id.is_empty() || !sku_id.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("Bambu store search result has no highlighted SKU".to_string());
    }
    let seo_code = record.get("seoCode").and_then(Value::as_str).unwrap_or_default();
    if seo_code.is_empty()
        || !seo_code
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err("Bambu store search result has an invalid product slug".to_string());
    }
    let image_url = record
        .get("mediaFiles")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .and_then(Value::as_str)
        .ok_or_else(|| "Bambu store search result has no highlighted product image".to_string())?;
    optimized_jpeg_url(image_url)
}

fn optimized_jpeg_url(url: &str) -> Result<String, String> {
    parse_bambu_cdn_url(url)?;
    let (without_query, query) = url.split_once('?').unwrap_or((url, ""));
    let base = without_query.split("__op__").next().unwrap_or(without_query);
    let mut optimized = format!("{base}__op__resize,m_lfit,w_240__op__format,f_jpg__op__quality,q_80");
    if !query.is_empty() {
        optimized.push('?');
        optimized.push_str(query);
    }
    Ok(optimized)
}

async fn download_product_image(stack: Stack<'static>, tls: TlsReference<'static>, url: &str) -> Result<Vec<u8>, String> {
    let (host, path) = parse_bambu_cdn_url(url)?;
    https_request(
        stack,
        tls,
        host,
        path,
        Method::Get,
        &[
            ("Host", host),
            ("Accept", "image/jpeg"),
            ("User-Agent", "FilaScan/0.1"),
            ("Connection", "close"),
        ],
        None,
        AMAZON_ROOT_CA_1,
        MAX_IMAGE_BYTES,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn https_request(
    stack: Stack<'static>,
    tls: TlsReference<'static>,
    host: &str,
    path: &str,
    method: Method,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
    root_ca: &str,
    max_bytes: usize,
) -> Result<Vec<u8>, String> {
    let ips = stack
        .dns_query(host, embassy_net::dns::DnsQueryType::A)
        .await
        .map_err(|error| format!("DNS lookup failed: {error:?}"))?;
    let Some(IpAddress::Ipv4(address)) = ips.first().copied() else {
        return Err("DNS lookup returned no IPv4 address".to_string());
    };

    let ca_pem = CString::new(root_ca).map_err(|_| "Embedded root certificate contains a null byte".to_string())?;
    let ca_chain = X509::pem(ca_pem.as_bytes_with_nul()).map_err(|error| format!("Invalid embedded root certificate: {error:?}"))?;
    let certificates = Certificates {
        ca_chain: Some(ca_chain),
        ..Default::default()
    };
    let mut tcp_buffers = Box::new(TcpBuffers::<1, 2048, 8192>::new());
    let tcp = Tcp::new(stack, &mut *tcp_buffers);
    let server_name = CString::new(host).map_err(|_| "Invalid HTTPS host".to_string())?;
    let tls_connector = Box::new(esp_mbedtls::asynch::TlsConnector::new(
        tcp,
        &server_name,
        TlsVersion::Tls1_2,
        certificates,
        tls,
    ));

    let mut connection_buffer = Box::new([0_u8; 4096]);
    let mut connection: Box<Connection<_, 32>> = Box::new(Connection::new(
        &mut *connection_buffer,
        &*tls_connector,
        SocketAddr::new(IpAddr::V4(address), 443),
    ));
    connection
        .initiate_request(true, method, path, headers)
        .await
        .map_err(|error| format!("HTTPS request failed: {error:?}"))?;
    if let Some(body) = body {
        connection
            .write_all(body)
            .await
            .map_err(|error| format!("HTTPS body write failed: {error:?}"))?;
    }
    connection
        .initiate_response()
        .await
        .map_err(|error| format!("HTTPS response failed: {error:?}"))?;

    let status = connection.headers().map_err(|error| format!("Invalid HTTP response: {error:?}"))?.code;
    if status != 200 {
        return Err(format!("HTTPS service returned HTTP {status}"));
    }

    let mut response = Vec::new();
    let mut chunk = [0_u8; 2048];
    loop {
        let length = connection
            .read(&mut chunk)
            .await
            .map_err(|error| format!("HTTPS response read failed: {error:?}"))?;
        if length == 0 {
            break;
        }
        if response.len() + length > max_bytes {
            return Err(format!("HTTPS response exceeds the {max_bytes} byte safety limit"));
        }
        response.extend_from_slice(&chunk[..length]);
    }
    Ok(response)
}

fn cache_path(product_code: &str) -> String {
    // Five digits plus a three-character extension fit FAT 8.3.
    format!("/filascan/images/{product_code}.jpg")
}

fn validate_product_code(product_code: &str) -> Result<(), String> {
    if product_code.len() != 5 || !product_code.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("Bambu product code is invalid".to_string());
    }
    Ok(())
}

fn parse_bambu_cdn_url(url: &str) -> Result<(&str, &str), String> {
    let rest = url.strip_prefix("https://").ok_or_else(|| "Image URL is not HTTPS".to_string())?;
    let (host, path_without_slash) = rest.split_once('/').ok_or_else(|| "Image URL has no path".to_string())?;
    if host != "store.bblcdn.eu" && host != "store.bblcdn.com" {
        return Err("Image URL uses an unsupported host".to_string());
    }
    let path_offset = url.len() - path_without_slash.len() - 1;
    Ok((host, &url[path_offset..]))
}

fn parse_openprinttag_image_url(url: &str) -> Result<(&str, &str), String> {
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| "OpenPrintTag image URL is not HTTPS".to_string())?;
    let (host, path_without_slash) = rest
        .split_once('/')
        .ok_or_else(|| "OpenPrintTag image URL has no path".to_string())?;
    if host != "files.openprinttag.org" {
        return Err("OpenPrintTag image URL uses an unsupported host".to_string());
    }
    let path_offset = url.len() - path_without_slash.len() - 1;
    let path = &url[path_offset..];
    if path.contains("/../") {
        return Err("OpenPrintTag image URL has an invalid path".to_string());
    }
    Ok((host, path))
}

fn decode_jpeg(encoded: &[u8]) -> Result<Image, String> {
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut decoder = JpegDecoder::new_with_options(encoded, options);
    let pixels = decoder.decode().map_err(|error| format!("JPEG decode failed: {error:?}"))?;
    let info = decoder.info().ok_or_else(|| "JPEG has no dimensions".to_string())?;
    if info.width == 0 || info.height == 0 || info.width > MAX_IMAGE_DIMENSION || info.height > MAX_IMAGE_DIMENSION {
        return Err(format!("Unsupported JPEG dimensions {}x{}", info.width, info.height));
    }

    let expected = info.width as usize * info.height as usize * 3;
    if pixels.len() != expected {
        return Err(format!("Unexpected JPEG pixel buffer size {}", pixels.len()));
    }
    let mut buffer = SharedPixelBuffer::<Rgb8Pixel>::new(info.width.into(), info.height.into());
    buffer.make_mut_bytes().copy_from_slice(&pixels);
    Ok(Image::from_rgb8(buffer))
}
