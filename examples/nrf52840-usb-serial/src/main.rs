#![no_main]
#![no_std]

use nrf52840_usb_serial as _; // global logger + panicking-behavior + memory layout
use nrf52840_hal::{
    clocks::Clocks,
    usbd::{UsbPeripheral, Usbd},
    pac::Peripherals,
    gpio::{p0::Parts, Level},
    prelude::OutputPin,
};
use usb_device::device::{UsbDeviceBuilder, UsbVidPid};
use usbd_serial::{SerialPort, USB_CLASS_CDC};
use groundhog::RollingTimer;
use groundhog_nrf52::GlobalRollingTimer;


#[cortex_m_rt::entry]
fn main() -> ! {
    let mut consumer = defmt_bbq::init().unwrap();

    defmt::println!("Hello, world!");

    let periph = Peripherals::take().unwrap();

    let clocks = Clocks::new(periph.CLOCK);
    let clocks = clocks.enable_ext_hfosc();

    GlobalRollingTimer::init(periph.TIMER0);
    let timer = GlobalRollingTimer::default();

    let p0 = Parts::new(periph.P0);

    let mut led1 = p0.p0_13.into_push_pull_output(Level::High);
    let mut led2 = p0.p0_14.into_push_pull_output(Level::High);
    let mut led3 = p0.p0_15.into_push_pull_output(Level::High);
    let mut led4 = p0.p0_16.into_push_pull_output(Level::High);

    let usb_bus = Usbd::new(UsbPeripheral::new(periph.USBD, &clocks));
    let mut serial = SerialPort::new(&usb_bus);

    let mut usb_dev = UsbDeviceBuilder::new(&usb_bus, UsbVidPid(0x16c0, 0x27dd))
        .manufacturer("Fake company")
        .product("Serial port")
        .serial_number("defmt-bbq")
        .device_class(USB_CLASS_CDC)
        .max_packet_size_0(64) // (makes control transfers 8x faster)
        .build();

    let start = timer.get_ticks();
    let mut last_ding = timer.get_ticks();
    let mut state = false;

    led3.set_low().ok();

    loop {
        if let Ok(grant) = consumer.read() {
            if state {
                led1.set_high().ok();
            } else {
                led1.set_low().ok();
            }

            state = !state;

            led2.set_low().ok();
            match serial.write(&grant) {
                Ok(len) if len > 0 => {
                    grant.release(len);
                }
                Ok(_) => {
                    // Let grant drop with no commit...
                }
                Err(_) => {
                    // Oops
                },
            }
        }

        if timer.millis_since(last_ding) >= 1000 {
            defmt::println!("Ding! - {=u32}", timer.millis_since(start));
            last_ding = timer.get_ticks();
        }

        if !usb_dev.poll(&mut [&mut serial]) {
            continue;
        } else {
            led4.set_low().ok();
        }
    }
}
