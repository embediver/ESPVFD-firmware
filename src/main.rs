use std::sync::mpsc::{channel, Sender};

use embedded_hal::spi::MODE_3;
use embedded_svc::mqtt::client::Event;
use embedded_svc::wifi::{AuthMethod, ClientConfiguration, Configuration};
use esp_idf_hal::delay::Delay;
use esp_idf_hal::gpio::{AnyIOPin, Gpio10, Gpio4, Gpio8, Output, PinDriver};
use esp_idf_hal::spi::SpiDriver;
use esp_idf_hal::spi::{
    config::{BitOrder, Config, DriverConfig},
    SpiDeviceDriver,
};
use esp_idf_hal::units::FromValueType;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::mqtt::client::{EspMqttClient, EspMqttMessage, MqttClientConfiguration};
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::EspWifi;
use esp_idf_sys as _; // If using the `binstart` feature of `esp-idf-sys`, always keep this module imported
use hcs_12ss59t::HCS12SS59T;
use log::*;

type VFD<'a> = HCS12SS59T<
    SpiDeviceDriver<'a, SpiDriver<'a>>,
    PinDriver<'a, Gpio4, Output>,
    PinDriver<'a, Gpio10, Output>,
    Delay,
    PinDriver<'a, Gpio8, Output>,
>;

const WIFI_SSID: &str = env!("WIFI_SSID");
const WIFI_PASS: &str = env!("WIFI_PASS");
const MQTT_URI: &str = "mqtt://192.168.77.5";

fn main() -> anyhow::Result<()> {
    // It is necessary to call this function once. Otherwise some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    esp_idf_sys::link_patches();
    // Bind the log crate to the ESP Logging facilities
    esp_idf_svc::log::EspLogger::initialize_default();

    info!("Hello, world!");

    let perip = esp_idf_hal::peripherals::Peripherals::take().unwrap();

    let sys_loop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    let spi = perip.spi2;
    let sclk = perip.pins.gpio3;
    let data = perip.pins.gpio5;
    let cs = PinDriver::output(perip.pins.gpio8).unwrap();

    let n_rst = PinDriver::output(perip.pins.gpio4).unwrap();
    let n_vdon = PinDriver::output(perip.pins.gpio10).unwrap();

    let spi_conf = Config::default()
        .baudrate(1.MHz().into())
        .bit_order(BitOrder::LsbFirst)
        .data_mode(MODE_3);

    let spi = SpiDeviceDriver::new_single(
        spi,
        sclk,
        data,
        Option::<AnyIOPin>::None,
        Option::<AnyIOPin>::None,
        &DriverConfig::default(),
        &spi_conf,
    )
    .unwrap();

    let delay = Delay;

    let mut vfd = HCS12SS59T::new(spi, n_rst, delay, Some(n_vdon), cs);

    vfd.init().unwrap();
    vfd.display("Hello World!").unwrap();

    // WIFI
    let mut wifi = EspWifi::new(perip.modem, sys_loop.clone(), Some(nvs))?;

    connect_wifi(&mut wifi, &mut vfd)?;
    info!("Wifi connected");

    // MQTT
    let (tx, rx) = channel();

    let conf = MqttClientConfiguration::default();
    let mut mqtt_client = EspMqttClient::new(MQTT_URI, &conf, move |message| {
        info!("{:?}", message);
        match message {
            Ok(Event::Received(m)) => match handle_mqtt_message(m, &tx) {
                Err(e) => info!("Error handling mqtt message: {e:?}"),
                _ => {}
            },
            _ => {}
        }
    })
    .unwrap();
    mqtt_client.subscribe("vfd/set-text", embedded_svc::mqtt::client::QoS::AtMostOnce)?;
    info!("MQTT initialized");

    info!("Should display \"DisplayReady\" now.");
    vfd.display("DisplayReady").unwrap();
    loop {
        let text = rx.recv()?;
        vfd.display(&text).unwrap();
    }
}

fn connect_wifi(wifi: &mut EspWifi<'static>, vfd: &mut VFD<'_>) -> anyhow::Result<()> {
    let wifi_configuration: Configuration = Configuration::Client(ClientConfiguration {
        ssid: WIFI_SSID.into(),
        bssid: None,
        auth_method: AuthMethod::WPA2Personal,
        password: WIFI_PASS.into(),
        channel: None,
    });

    wifi.set_configuration(&wifi_configuration)?;

    let mut load_i: usize = 0;
    wifi.start()?;
    while !wifi.is_started()? {
        let mut s = "OOOOOOOOOOOO".to_owned();
        s.replace_range(load_i..load_i + 1, "*");
        vfd.display(&s).unwrap();
        Delay::delay_ms(200);
        load_i += 1;
        load_i = load_i % 12;
        // vfd.display("080808080808").unwrap();
        // Delay::delay_ms(500);
    }
    info!("Wifi started");

    wifi.connect()?;
    while !wifi.is_connected()? {
        let mut s = "OOOOOOOOOOOO".to_owned();
        s.replace_range(load_i..load_i + 1, "*");
        vfd.display(&s).unwrap();
        Delay::delay_ms(200);
        load_i += 1;
        load_i = load_i % 12;
    }
    info!("Wifi connected");

    // wifi.wait_netif_up()?;
    while !wifi.is_up()? {
        let mut s = "OOOOOOOOOOOO".to_owned();
        s.replace_range(load_i..load_i + 1, "*");
        vfd.display(&s).unwrap();
        Delay::delay_ms(200);
        load_i += 1;
        load_i = load_i % 12;
    }
    info!("Wifi netif up");
    vfd.display("connected   ").unwrap();
    Delay::delay_ms(1000);

    Ok(())
}

fn handle_mqtt_message(message: &EspMqttMessage, tx: &Sender<String>) -> anyhow::Result<()> {
    match message.topic() {
        Some("vfd/set-text") => {
            let buf = message.data();
            let s = String::from_utf8_lossy(buf);
            tx.send(s.into_owned())?;
            Ok(())
        }
        _ => {
            // not of interest
            Ok(())
        }
    }
}
