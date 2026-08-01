#![no_std]
#![no_main]
#![feature(asm_experimental_arch)]
#![feature(type_alias_impl_trait)]
#![feature(impl_trait_in_assoc_type)]
#![recursion_limit = "256"]

mod app;
mod bambu_spool;
mod diagnostics;
mod settings;
mod web_app;

extern crate alloc;

use alloc::rc::Rc;
use core::{cell::RefCell, marker::PhantomData, net::Ipv4Addr};

use embassy_embedded_hal::adapter::BlockingAsync;
use embassy_executor::Spawner;
use embassy_net::{Ipv4Cidr, StackResources, StaticConfigV4};
use embassy_time::{Duration, Timer};
use esp_alloc::{self as _, HeapStats};
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    dma::DmaTxBuf,
    dma_buffers,
    gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull},
    rng::Rng,
    rtc_cntl::Rtc,
    spi::{self, master::Spi},
    time::Rate,
    timer::timg::TimerGroup,
};
use esp_mbedtls::Tls;
use esp_storage::FlashStorage;
use framework::{
    RNG,
    framework::FrameworkSettings,
    prelude::*,
    wt32_sc01_plus::{WT32SC01Plus, WT32SC01PlusDisplayPeripherals, WT32SC01PlusRunner, WT32SC01PlusSDCardPeripherals},
};
use framework_macros::include_bytes_gz;
use slint::ComponentHandle;

use settings::*;
use web_app::WifiAppBuilder;

const STA_STACK_RESOURCES: usize = WEB_SERVER_NUM_LISTENERS + FRAMEWORK_STA_STACK_RESOURCES;
const AP_STACK_RESOURCES: usize = WEB_SERVER_NUM_LISTENERS + FRAMEWORK_AP_STACK_RESOURCES;

esp_bootloader_esp_idf::esp_app_desc!();

fn init_psram_heap(start: *mut u8, size: usize) {
    unsafe {
        esp_alloc::HEAP.add_region(esp_alloc::HeapRegion::new(start, size, esp_alloc::MemoryCapability::External.into()));
    }
}

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    esp_println::logger::init_logger_from_env();
    info!("FilaScan starting");

    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));

    #[allow(static_mut_refs)]
    unsafe {
        RNG.set(Rng::new()).ok();
    }

    let (psram_start, psram_size) = esp_hal::psram::psram_raw_parts(&peripherals.PSRAM);
    init_psram_heap(psram_start, psram_size);
    esp_alloc::heap_allocator!(size: 122 * 1024);
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 64 * 1024);
    spawner.spawn_heap(heap_stats_task()).ok();

    let _rtc = Rtc::new(peripherals.LPWR);
    let timer_group = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timer_group.timer1);

    let tls = mk_static!(Tls<'static>, Tls::new(peripherals.SHA).unwrap().with_hardware_rsa(peripherals.RSA));
    tls.set_debug(0);

    let flash_map = FlashMap::new_in_region(BlockingAsync::new(FlashStorage::new()), "map", 4096, env!("CARGO_PKG_NAME"))
        .await
        .expect("Failed to initialize flash map");
    let flash_map = Rc::new(RefCell::new(flash_map));

    let radio = &*mk_static!(esp_radio::Controller<'static>, esp_radio::init().unwrap());
    let (wifi_controller, wifi_interfaces) = esp_radio::wifi::new(radio, peripherals.WIFI, Default::default()).unwrap();

    let seed_rng = Rng::new();
    let seed = (seed_rng.random() as u64) << 32 | seed_rng.random() as u64;
    let (sta_stack, sta_runner) = embassy_net::new(
        wifi_interfaces.sta,
        embassy_net::Config::dhcpv4(Default::default()),
        mk_static!(StackResources<STA_STACK_RESOURCES>, StackResources::new()),
        seed,
    );
    let ap_config = embassy_net::Config::ipv4_static(StaticConfigV4 {
        address: Ipv4Cidr::new(Ipv4Addr::new(AP_ADDR.0, AP_ADDR.1, AP_ADDR.2, AP_ADDR.3), 24),
        gateway: Some(Ipv4Addr::new(AP_ADDR.0, AP_ADDR.1, AP_ADDR.2, AP_ADDR.3)),
        dns_servers: Default::default(),
    });
    let (ap_stack, ap_runner) = embassy_net::new(
        wifi_interfaces.ap,
        ap_config,
        mk_static!(StackResources<AP_STACK_RESOURCES>, StackResources::new()),
        seed,
    );

    let framework_settings = FrameworkSettings {
        ota_domain: OTA_DOMAIN,
        ota_path: OTA_PATH,
        ota_toml_filename: OTA_TOML_FILENAME,
        ota_certs: OTA_TLS_CERTIFICATE,
        ap_addr: AP_ADDR,
        web_server_https: WEB_SERVER_HTTPS,
        web_server_port: WEB_SERVER_PORT,
        web_server_captive: WEB_SERVER_CAPTIVE,
        web_server_num_listeners: WEB_SERVER_NUM_LISTENERS,
        web_server_tls_certificate: WEB_SERVER_TLS_CERTIFICATE,
        web_server_tls_private_key: WEB_SERVER_TLS_PRIVATE_KEY,
        web_app_domain: WEB_APP_DOMAIN,
        web_app_security_key_length: WEB_APP_SECURITY_KEY_LENGTH,
        web_app_salt: WEB_APP_SALT,
        web_app_key_derivation_iterations: WEB_APP_KEY_DERIVATION_ITERATIONS,
        app_cargo_pkg_name: env!("CARGO_PKG_NAME"),
        app_cargo_pkg_version: env!("CARGO_PKG_VERSION"),
        default_fixed_security_key: None,
        mdns: true,
        ntp: false,
    };
    let framework = Framework::new(framework_settings, flash_map, spawner, sta_stack, tls.reference(), None);
    framework
        .borrow_mut()
        .load_config_flash_then_toml("")
        .expect("Failed to load FilaScan settings");

    let display_peripherals = WT32SC01PlusDisplayPeripherals {
        GPIO47: peripherals.GPIO47,
        GPIO0: peripherals.GPIO0,
        GPIO45: peripherals.GPIO45,
        GPIO4: peripherals.GPIO4,
        LCD_CAM: peripherals.LCD_CAM,
        GPIO9: peripherals.GPIO9,
        GPIO46: peripherals.GPIO46,
        GPIO3: peripherals.GPIO3,
        GPIO8: peripherals.GPIO8,
        GPIO18: peripherals.GPIO18,
        GPIO17: peripherals.GPIO17,
        GPIO16: peripherals.GPIO16,
        GPIO15: peripherals.GPIO15,
        LEDC: peripherals.LEDC,
        GPIO5: peripherals.GPIO5,
        GPIO6: peripherals.GPIO6,
        GPIO7: peripherals.GPIO7,
        DMA_CHx: peripherals.DMA_CH0,
        I2Cx: peripherals.I2C0,
    };
    let sdcard_peripherals = WT32SC01PlusSDCardPeripherals {
        GPIO38: peripherals.GPIO38,
        GPIO39: peripherals.GPIO39,
        GPIO40: peripherals.GPIO40,
        GPIO41: peripherals.GPIO41,
        SPIx: peripherals.SPI3,
        DMA_CHx: peripherals.DMA_CH2,
    };
    let orientation = mipidsi::options::Orientation::new()
        .rotate(mipidsi::options::Rotation::Deg270)
        .flip_horizontal();
    let (display, display_runner, _unused_sdcard) = WT32SC01Plus::new(display_peripherals, sdcard_peripherals, orientation, framework.clone());
    spawner.spawn(display_task(display_runner)).ok();
    display.wait_init_done().await.ok();

    let ui = mk_static!(app::AppWindow, app::create_slint_app());
    let diagnostics = Rc::new(RefCell::new(diagnostics::LogBuffer::new()));
    diagnostics.borrow_mut().info("FilaScan diagnostics started");

    let (rx_buffer, rx_descriptors, tx_buffer, tx_descriptors) = dma_buffers!(64);
    let spi_rx = esp_hal::dma::DmaRxBuf::new(rx_descriptors, rx_buffer).unwrap();
    let spi_tx = DmaTxBuf::new(tx_descriptors, tx_buffer).unwrap();
    let pn532_irq = Input::new(peripherals.GPIO14, InputConfig::default().with_pull(Pull::None));
    let spi = Spi::new(
        peripherals.SPI2,
        esp_hal::spi::master::Config::default()
            .with_frequency(Rate::from_khz(2000))
            .with_mode(spi::Mode::_0)
            .with_read_bit_order(spi::BitOrder::LsbFirst)
            .with_write_bit_order(spi::BitOrder::LsbFirst),
    )
    .unwrap()
    .with_sck(peripherals.GPIO13)
    .with_mosi(Output::new(peripherals.GPIO11, Level::High, OutputConfig::default()))
    .with_miso(peripherals.GPIO12)
    .with_dma(peripherals.DMA_CH1)
    .with_buffers(spi_rx, spi_tx)
    .into_async();
    let pn532_spi = embedded_hal_bus::spi::ExclusiveDevice::new(
        spi,
        Output::new(peripherals.GPIO10, Level::High, OutputConfig::default()),
        embassy_time::Delay,
    )
    .unwrap();
    let _reader_controller = app::init_app(ui.as_weak(), framework.clone(), diagnostics.clone(), pn532_spi, pn532_irq);

    let web_state_data = web_app::FilaScanWebState {
        diagnostics: diagnostics.clone(),
    };
    let web_builder = framework::framework_web_app::WebAppBuilder::<web_app::FilaScanWebState, WifiAppBuilder> {
        framework: framework.clone(),
        captive_html_gz: include_bytes_gz!("static/captive.html"),
        web_app_html_gz: include_bytes_gz!("static/config.html"),
        app_builder: WifiAppBuilder { captive: WEB_SERVER_CAPTIVE },
        _phantom: PhantomData,
    };
    let router = mk_static!(
        picoserve::AppRouter<framework::framework_web_app::WebAppBuilder<web_app::FilaScanWebState, WifiAppBuilder>>,
        picoserve::AppWithStateBuilder::build_app(web_builder)
    );
    let web_state = mk_static!(
        framework::framework_web_app::WebAppState<web_app::FilaScanWebState>,
        framework::framework_web_app::WebAppState::new(framework.borrow().encryption_key, framework.clone(), web_state_data,)
    );
    let web_config = picoserve::Config::new(picoserve::Timeouts {
        start_read_request: Some(Duration::from_secs(5)),
        read_request: Some(Duration::from_secs(5)),
        write: Some(Duration::from_secs(5)),
    })
    .keep_connection_alive();
    let web_runner = mk_static!(
        framework::web_server::WebAppRunner<web_app::FilaScanWebState, WifiAppBuilder>,
        framework::web_server::WebAppRunner::new(
            framework.clone(),
            router,
            web_state,
            web_config,
        )
    );
    for listener in 0..WEB_SERVER_NUM_LISTENERS {
        spawner.spawn_heap(web_server_task(web_runner, listener)).unwrap();
    }

    let (usb_rx, usb_tx) = esp_hal::usb_serial_jtag::UsbSerialJtag::new(peripherals.USB_DEVICE).into_async().split();
    spawner
        .spawn_heap(framework::wifi::connection_task_inner(
            wifi_controller,
            sta_stack,
            ap_stack,
            usb_rx,
            usb_tx,
            framework.clone(),
        ))
        .ok();
    spawner.spawn(framework::wifi::sta_net_task(sta_runner)).ok();
    spawner.spawn(framework::wifi::ap_net_task(ap_runner)).ok();

    framework.borrow().notify_initialization_completed(true);
    Framework::wait_for_wifi(&framework).await;
    framework.borrow_mut().start_web_app(sta_stack, framework::framework::WebConfigMode::STA);

    loop {
        Timer::after_secs(60).await;
    }
}

async fn web_server_task(runner: &'static framework::web_server::WebAppRunner<web_app::FilaScanWebState, WifiAppBuilder>, listener: usize) {
    runner.run(listener).await;
}

#[embassy_executor::task]
async fn display_task(mut runner: WT32SC01PlusRunner<esp_hal::peripherals::DMA_CH0<'static>, esp_hal::peripherals::I2C0<'static>>) {
    runner.run().await;
}

async fn heap_stats_task() {
    loop {
        let stats: HeapStats = esp_alloc::HEAP.stats();
        debug!("{}", stats);
        Timer::after_secs(30).await;
    }
}
