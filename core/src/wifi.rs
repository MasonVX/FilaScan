use alloc::{rc::Rc, string::String, vec};
use core::{
    cell::RefCell,
    net::{IpAddr, Ipv4Addr, SocketAddr},
};

use edge_dhcp::io::{self, DEFAULT_SERVER_PORT};
use edge_nal::UdpBind;
use embassy_net::Stack;
use embassy_time::{Duration, Instant, Timer, with_timeout};
use esp_radio::wifi::{AccessPointConfig, ClientConfig, ModeConfig, WifiController};
use framework::{
    framework::{Framework, WebConfigMode},
    utils::SpawnerHeapExt,
};

const BOOT_WIFI_TIMEOUT: Duration = Duration::from_secs(60);
const RETRY_DELAY: Duration = Duration::from_secs(1);

#[allow(clippy::too_many_arguments)]
pub async fn connection_task_with_boot_fallback(
    mut controller: WifiController<'static>,
    sta_stack: Stack<'static>,
    ap_stack: Stack<'static>,
    rx: esp_hal::usb_serial_jtag::UsbSerialJtagRx<'static, esp_hal::Async>,
    tx: esp_hal::usb_serial_jtag::UsbSerialJtagTx<'static, esp_hal::Async>,
    framework: Rc<RefCell<Framework>>,
) {
    let credentials = {
        let framework = framework.borrow();
        framework.wifi_ssid.clone().zip(framework.wifi_password.clone())
    };

    let Some((ssid, password)) = credentials else {
        framework::wifi::connection_task_inner(controller, sta_stack, ap_stack, rx, tx, framework).await;
        return;
    };

    // USB Improv is used only by the framework's initial provisioning path.
    // Keep ownership explicit in this task without holding an active session.
    drop((rx, tx));

    let client_config = ClientConfig::default()
        .with_ssid(ssid.clone())
        .with_password(password);
    controller.set_config(&ModeConfig::Client(client_config.clone())).unwrap();
    if !matches!(controller.is_started(), Ok(true)) {
        controller.start_async().await.unwrap();
    }

    let boot_started = Instant::now();
    loop {
        let elapsed = boot_started.elapsed();
        if elapsed >= BOOT_WIFI_TIMEOUT {
            start_setup_access_point(&mut controller, ap_stack, framework.clone()).await;
            return;
        }

        let remaining = BOOT_WIFI_TIMEOUT - elapsed;
        match with_timeout(remaining, controller.connect_async()).await {
            Ok(Ok(())) => {
                if wait_for_ipv4_until(sta_stack, boot_started, BOOT_WIFI_TIMEOUT).await {
                    report_connected(sta_stack, &ssid, &framework);
                    maintain_station_connection(&mut controller, sta_stack, &ssid, &framework).await;
                    return;
                }
            }
            Ok(Err(error)) => {
                log::warn!("Wi-Fi connection attempt failed during boot: {error:?}");
            }
            Err(_) => {}
        }

        if boot_started.elapsed() < BOOT_WIFI_TIMEOUT {
            Timer::after(RETRY_DELAY).await;
        }
    }
}

async fn wait_for_ipv4_until(stack: Stack<'static>, started: Instant, timeout: Duration) -> bool {
    let mut link_seen = false;
    loop {
        if stack.config_v4().is_some() {
            return true;
        }
        if started.elapsed() >= timeout {
            return false;
        }
        if stack.is_link_up() {
            link_seen = true;
        } else if link_seen {
            return false;
        }
        Timer::after_millis(250).await;
    }
}

async fn maintain_station_connection(
    controller: &mut WifiController<'static>,
    stack: Stack<'static>,
    ssid: &str,
    framework: &Rc<RefCell<Framework>>,
) {
    loop {
        while stack.is_link_up() && stack.config_v4().is_some() {
            Timer::after_secs(1).await;
        }

        framework.borrow_mut().report_wifi(None, false, ssid);
        framework.borrow().notify_wifi_sta_disconnected();
        log::warn!("Wi-Fi connection lost; retrying without enabling the setup access point");
        let _ = with_timeout(Duration::from_secs(5), controller.disconnect_async()).await;

        loop {
            match controller.connect_async().await {
                Ok(()) => {
                    if wait_for_ipv4_until(stack, Instant::now(), Duration::from_secs(15)).await {
                        report_connected(stack, ssid, framework);
                        break;
                    }
                }
                Err(error) => log::warn!("Wi-Fi reconnection attempt failed: {error:?}"),
            }
            Timer::after(RETRY_DELAY).await;
        }
    }
}

fn report_connected(stack: Stack<'static>, ssid: &str, framework: &Rc<RefCell<Framework>>) {
    if let Some(config) = stack.config_v4() {
        log::info!("Wi-Fi connected with IP {}", config.address);
        framework
            .borrow_mut()
            .report_wifi(Some(config.address.address()), false, ssid);
        framework.borrow().notify_wifi_sta_connected();
    }
}

async fn start_setup_access_point(
    controller: &mut WifiController<'static>,
    ap_stack: Stack<'static>,
    framework: Rc<RefCell<Framework>>,
) {
    let (ap_name, ap_addr, captive, spawner) = {
        let framework = framework.borrow();
        (
            String::from(framework.settings.app_cargo_pkg_name),
            framework.settings.ap_addr,
            framework.settings.web_server_captive,
            framework.spawner,
        )
    };
    let access_point_config = AccessPointConfig::default().with_ssid(ap_name.clone());
    controller
        .set_config(&ModeConfig::AccessPoint(access_point_config))
        .unwrap();
    if !matches!(controller.is_started(), Ok(true)) {
        controller.start_async().await.unwrap();
    }

    spawner.spawn_heap(dhcp_server(ap_stack, framework.clone())).ok();
    if captive {
        spawner.spawn_heap(captive_portal(ap_stack, framework.clone())).ok();
    }
    Timer::after_secs(1).await;
    framework.borrow_mut().start_web_app(ap_stack, WebConfigMode::AP);
    framework.borrow_mut().report_wifi(
        Some(Ipv4Addr::new(ap_addr.0, ap_addr.1, ap_addr.2, ap_addr.3)),
        true,
        &ap_name,
    );
    log::warn!(
        "Wi-Fi was not available within 60 seconds; setup access point '{}' started at {}.{}.{}.{}",
        ap_name,
        ap_addr.0,
        ap_addr.1,
        ap_addr.2,
        ap_addr.3
    );

    loop {
        Timer::after_secs(60).await;
    }
}

// These small server adapters follow the esp-hal-app-framework 0.6.1 setup
// path, which is licensed under MIT OR Apache-2.0.
async fn dhcp_server(stack: Stack<'static>, framework: Rc<RefCell<Framework>>) {
    let ap_addr = framework.borrow().settings.ap_addr;
    let address = Ipv4Addr::new(ap_addr.0, ap_addr.1, ap_addr.2, ap_addr.3);
    let mut server: edge_dhcp::server::Server<fn() -> u64, 3> =
        edge_dhcp::server::Server::new_with_et(address);
    let mut gateways = [address];
    let mut options = edge_dhcp::server::ServerOptions::new(address, Some(&mut gateways));
    let dns_servers = [address];
    options.dns = &dns_servers;

    let mut buffer = vec![0; 512];
    let udp_buffers: edge_nal_embassy::UdpBuffers<1, 512, 512, 1> =
        edge_nal_embassy::UdpBuffers::new();
    let udp = edge_nal_embassy::Udp::new(stack, &udp_buffers);
    let address = core::net::SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DEFAULT_SERVER_PORT);
    let mut socket = udp.bind(core::net::SocketAddr::V4(address)).await.unwrap();
    io::server::server::run(&mut server, &options, &mut socket, &mut buffer)
        .await
        .unwrap();
}

async fn captive_portal(stack: Stack<'static>, framework: Rc<RefCell<Framework>>) {
    let ap_addr = framework.borrow().settings.ap_addr;
    let udp_buffers: edge_nal_embassy::UdpBuffers<1, 512, 512, 1> =
        edge_nal_embassy::UdpBuffers::new();
    let udp = edge_nal_embassy::Udp::new(stack, &udp_buffers);
    let mut tx_buffer = vec![0; 512];
    let mut rx_buffer = vec![0; 512];
    edge_captive::io::run(
        &udp,
        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 53),
        &mut tx_buffer,
        &mut rx_buffer,
        Ipv4Addr::new(ap_addr.0, ap_addr.1, ap_addr.2, ap_addr.3),
        core::time::Duration::from_secs(60),
    )
    .await
    .unwrap();
}
