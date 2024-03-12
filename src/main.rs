use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use embedded_hal::spi::MODE_3;
use embedded_svc::wifi::{AuthMethod, ClientConfiguration, Configuration};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::delay::Delay;
use esp_idf_svc::hal::gpio::{AnyIOPin, Gpio2, Gpio4, Gpio5, Output, PinDriver};
use esp_idf_svc::hal::spi::SpiDriver;
use esp_idf_svc::hal::spi::{
    config::{BitOrder, Config, DriverConfig},
    SpiDeviceDriver,
};
use esp_idf_svc::hal::units::FromValueType;
use esp_idf_svc::mqtt::client::{EspMqttClient, MqttClientConfiguration};
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::EspWifi;
use hcs_12ss59t::{animation::mode, animation::ScrollingText, HCS12SS59T};

use log::*;

// Type to easy pass our concrete VFD imlementation around
type Vfd<'a> = HCS12SS59T<
    SpiDeviceDriver<'a, SpiDriver<'a>>,
    PinDriver<'a, Gpio4, Output>,
    PinDriver<'a, Gpio2, Output>,
    Delay,
    PinDriver<'a, Gpio5, Output>,
>;

const WIFI_SSID: &str = env!("WIFI_SSID");
const WIFI_PASS: &str = env!("WIFI_PASS");
// const MQTT_URI: &str = "mqtt://mqtt.42volt.de";
const MQTT_URI: Option<&str> = option_env!("MQTT_URI");
const MQTT_USER: Option<&str> = option_env!("MQTT_USER");
const MQTT_PASS: Option<&str> = option_env!("MQTT_PASS");

fn main() -> ! {
    app().unwrap();
    unreachable!()
}

fn app() -> anyhow::Result<()> {
    // It is necessary to call this function once. Otherwise some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    esp_idf_svc::sys::link_patches();
    // Bind the log crate to the ESP Logging facilities
    esp_idf_svc::log::EspLogger::initialize_default();

    info!("Initializing...");

    // First get some peripheral access
    let perip = esp_idf_svc::hal::peripherals::Peripherals::take().unwrap();

    let sys_loop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    // set up SPI for the VFD
    let spi = perip.spi2;
    let sclk = perip.pins.gpio6;
    let data = perip.pins.gpio7;
    let cs = PinDriver::output(perip.pins.gpio5).unwrap();

    let n_rst = PinDriver::output(perip.pins.gpio4).unwrap();
    let n_vdon = PinDriver::output(perip.pins.gpio2).unwrap();

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

    let delay = Delay::new_default();

    // Initialize the VFD
    let mut vfd = HCS12SS59T::new(spi, n_rst, delay, Some(n_vdon), cs);

    vfd.init().unwrap();
    vfd.display("Initializing".chars()).unwrap();

    // WIFI
    let mut wifi = EspWifi::new(perip.modem, sys_loop.clone(), Some(nvs))?;

    connect_wifi(&mut wifi, &mut vfd)?;
    info!("Wifi connected");

    // Get and display MAC
    let mac = wifi.get_mac(esp_idf_svc::wifi::WifiDeviceId::Sta)?;
    let device_id = format!("{:02X}{:02X}{:02X}", mac[3], mac[4], mac[5]);
    info!("Device ID: {}", device_id);
    {
        let text = format!("ID {}", device_id);
        vfd.display(text.chars()).unwrap();
        delay.delay_ms(5000);
    }

    // MQTT
    let (tx, rx) = channel();
    let mqtt_crashed = Arc::new(AtomicBool::new(false));
    let mqtt_crash_notifier = mqtt_crashed.clone();

    let conf = MqttClientConfiguration {
        username: MQTT_USER,
        password: MQTT_PASS,
        ..Default::default()
    };
    let mqtt_uri = MQTT_URI.unwrap_or("mqtt://mqtt.skynt.de");
    let mut mqtt_client = EspMqttClient::new_cb(mqtt_uri, &conf, move |message| {
        info!("{:?}", message.payload());
        match message.payload() {
            esp_idf_svc::mqtt::client::EventPayload::Received {
                id: _,
                topic,
                data,
                details: _,
            } => handle_mqtt_message(topic, data, &tx).unwrap(),
            esp_idf_svc::mqtt::client::EventPayload::Error(_) => {
                mqtt_crash_notifier.store(true, std::sync::atomic::Ordering::Relaxed)
            }
            esp_idf_svc::mqtt::client::EventPayload::Disconnected => {
                mqtt_crash_notifier.store(true, std::sync::atomic::Ordering::Relaxed)
            }
            _ => {}
        }
    })
    .unwrap();

    let main_topic = format!("vfd-{}/", device_id);
    mqtt_client.subscribe(
        &format!("{}set-text", main_topic),
        embedded_svc::mqtt::client::QoS::AtMostOnce,
    )?;
    info!("MQTT: subscribed to {}set-text", main_topic);
    info!("MQTT initialized");

    // Initialization done turn off display and continue with main loop
    vfd.vd_off().unwrap();

    let mut text = String::new();
    let mut scroller = ScrollingText::new(&text, false, mode::Cycle);
    loop {
        if let Ok(t) = rx.recv_timeout(Duration::from_millis(500)) {
            if t == text {
                continue;
            }
            if t.chars().all(|c| matches!(c, '.' | ',' | ' ')) {
                // if all chars are matching one of whitespace chars, turn off display
                vfd.vd_off().unwrap();
            } else {
                vfd.vd_on().unwrap();
            }
            text.clear();
            text.push_str(&t);
            if t.len() < 12 {
                text.extend(core::iter::repeat('.').take(12 - t.len()));
            }
            scroller = ScrollingText::new(&text, false, mode::Cycle);
        }
        vfd.display(scroller.get_next()).unwrap();
        if mqtt_crashed.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(anyhow!("Mqtt error/disconnected"));
        }
    }
}

fn connect_wifi(wifi: &mut EspWifi<'static>, vfd: &mut Vfd<'_>) -> anyhow::Result<()> {
    let delay = Delay::new_default();
    let wifi_configuration: Configuration = Configuration::Client(ClientConfiguration {
        ssid: WIFI_SSID.try_into().unwrap(),
        bssid: None,
        auth_method: AuthMethod::WPA2Personal,
        password: WIFI_PASS.try_into().unwrap(),
        channel: None,
    });

    wifi.set_configuration(&wifi_configuration)?;

    let mut load_i: usize = 0;
    wifi.stop()?; // Try to stop WiFi first to ensure its in a clean state
    while wifi.is_started()? {
        loading_animation(&mut load_i, vfd, &delay);
    }
    wifi.start()?;
    while !wifi.is_started()? {
        loading_animation(&mut load_i, vfd, &delay);
    }
    info!("Wifi started");

    wifi.connect()?;
    while !wifi.is_connected()? {
        loading_animation(&mut load_i, vfd, &delay);
    }
    info!("Wifi connected");

    // wifi.wait_netif_up()?;
    while !wifi.is_up()? {
        loading_animation(&mut load_i, vfd, &delay);
    }
    info!("Wifi netif up");
    vfd.display("connected   ".chars()).unwrap();
    delay.delay_ms(1000);

    Ok(())
}

fn loading_animation(i: &mut usize, vfd: &mut Vfd<'_>, delay: &Delay) {
    let mut s = "OOOOOOOOOOOO".to_owned();
    s.replace_range(*i..*i + 1, "*");
    vfd.display(s.chars()).unwrap();
    delay.delay_ms(200);
    *i += 1;
    *i %= 12;
}

fn handle_mqtt_message(
    topic: Option<&str>,
    data: &[u8],
    tx: &Sender<String>,
) -> anyhow::Result<()> {
    if let Some(topic) = topic {
        if topic.contains("set-text") {
            let s = String::from_utf8_lossy(data);
            tx.send(s.into_owned())?;
        }
    }
    Ok(())
}
