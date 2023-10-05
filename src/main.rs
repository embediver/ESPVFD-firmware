use embedded_hal::spi::MODE_3;
use esp_idf_hal::delay::Delay;
use esp_idf_hal::gpio::{AnyIOPin, PinDriver};
use esp_idf_hal::spi::{
    config::{BitOrder, Config, DriverConfig},
    SpiDeviceDriver,
};
use esp_idf_hal::units::FromValueType;
use esp_idf_sys as _; // If using the `binstart` feature of `esp-idf-sys`, always keep this module imported
use hcs_12ss59t::HCS12SS59T;
use log::*;

fn main() {
    // It is necessary to call this function once. Otherwise some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    esp_idf_sys::link_patches();
    // Bind the log crate to the ESP Logging facilities
    esp_idf_svc::log::EspLogger::initialize_default();

    info!("Hello, world!");

    let perip = esp_idf_hal::peripherals::Peripherals::take().unwrap();

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
    info!("Should display \"Hello World!\" now.");
    loop {
        Delay::delay_ms(500);
        vfd.display("Hello World!").unwrap();
    }
}
