#![no_std]
#![no_main]

use bleps::{
    ad_structure::{
        create_advertising_data, AdStructure, BR_EDR_NOT_SUPPORTED, LE_GENERAL_DISCOVERABLE,
    },
    attribute_server::{AttributeServer, NotificationData, WorkResult},
    gatt, Ble, HciConnector,
};
use esp_backtrace as _;
use esp_println::println;
use esp_wifi::{ble::controller::BleConnector, initialize, EspWifiInitFor};
use hal::{
    clock::ClockControl,
    gpio::IO,
    peripherals::Peripherals,
    prelude::*,
    spi::SpiMode,
    xtensa_lx::mutex::{CriticalSectionMutex, Mutex},
    Delay, Spi,
};
use mfrc522::{comm::eh02::spi::SpiInterface, Mfrc522};

#[entry]
fn main() -> ! {
    let peripherals = Peripherals::take();
    let mut system = peripherals.DPORT.split();
    let clocks =
        ClockControl::configure(system.clock_control, hal::clock::CpuClock::Clock240MHz).freeze();

    let io = IO::new(peripherals.GPIO, peripherals.IO_MUX);
    let mut led = io.pins.gpio12.into_push_pull_output();
    let mut led2 = io.pins.gpio13.into_push_pull_output();

    led.set_high().unwrap();

    let spi = Spi::new(
        peripherals.SPI2,
        io.pins.gpio18,
        io.pins.gpio23,
        io.pins.gpio19,
        io.pins.gpio21,
        fugit::HertzU32::Hz(1000),
        SpiMode::Mode0,
        &mut system.peripheral_clock_control,
        &clocks,
    );

    let itf = SpiInterface::new(spi);
    let mut reader = Mfrc522::new(itf).init().unwrap();

    let init = initialize(
        EspWifiInitFor::Ble,
        hal::timer::TimerGroup::new(
            peripherals.TIMG1,
            &clocks,
            &mut system.peripheral_clock_control,
        )
        .timer0,
        hal::Rng::new(peripherals.RNG),
        system.radio_clock_control,
        &clocks,
    )
    .unwrap();

    let (_, mut bluetooth) = peripherals.RADIO.split();

    'main: {
        let v = reader.version().unwrap();
        if v != 0x91 && v != 0x92 {
            println!("could not find reader\nversion: {v}");
            led.toggle().unwrap();
            led2.toggle().unwrap();
            break 'main;
        }
        let reader = CriticalSectionMutex::new(reader);

        let ble_connector = BleConnector::new(&init, &mut bluetooth);
        let hci = HciConnector::new(ble_connector, esp_wifi::current_millis);
        let mut ble = Ble::new(&hci);

        println!("{:?}", ble.init());
        println!("{:?}", ble.cmd_set_le_advertising_parameters());
        println!(
            "{:?}",
            ble.cmd_set_le_advertising_data(
                create_advertising_data(&[
                    AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
                    AdStructure::ServiceUuids16(&[Uuid::Uuid16(0x1809)]),
                    AdStructure::CompleteLocalName("ESP32"),
                ])
                .unwrap()
            )
        );
        println!("{:?}", ble.cmd_set_le_advertise_enable(true));
        println!("started advertising");

        let mut rf = |_: usize, data: &mut [u8]| {
            let mut size = 0;
            (&reader).lock(|reader| match reader.reqa() {
                Ok(atqa) => {
                    if let Ok(uid) = reader.select(&atqa) {
                        let uid = uid.as_bytes();
                        size = uid.len();
                        data.copy_from_slice(uid);
                    }
                }
                Err(err) => println!("{err:#?}"),
            });
            size
        };

        let mut wf = |_: usize, data: &[u8]| {
            assert!(data.len() == 16);
            let mut formatted_data = [0; 16];
            formatted_data.copy_from_slice(data);
            (&reader).lock(|reader| {
                if let Err(err) = reader.mf_write(0, formatted_data) {
                    println!("{err:?}");
                }
            });
        };

        gatt!([service {
            uuid: "937312e0-2354-11eb-9f10-fbc30a62cf38",
            characteristics: [characteristic {
                name: "main",
                uuid: "937312e0-2354-11eb-9f10-fbc30a62cf38",
                notify: true,
                read: rf,
                write: wf,
            },],
        },]);

        let mut srv = AttributeServer::new(&mut ble, &mut gatt_attributes);

        let mut delay = Delay::new(&clocks);
        loop {
            let mut notification = None;
            let mut cccd = [0u8; 1];
            if let Some(1) = srv.get_characteristic_value(main_notify_enable_handle, 0, &mut cccd) {
                if cccd[0] == 1 {
                    notification = Some(NotificationData::new(
                        main_notify_enable_handle,
                        &b"Notification"[..],
                    ));
                }
            }

            match srv.do_work_with_notification(notification) {
                Ok(res) => {
                    println!("{res:?}");
                    if let WorkResult::GotDisconnected = res {
                        break;
                    }
                }
                Err(err) => {
                    println!("{:x?}", err);
                }
            }

            (&reader).lock(|reader| match reader.reqa() {
                Ok(atqa) => {
                    if let Ok(uid) = reader.select(&atqa) {
                        println!("{:?}", uid.as_bytes());
                    }
                }
                Err(err) => println!("{err:#?}"),
            });

            delay.delay_ms(1000u32);
            led2.toggle().unwrap();
            led.toggle().unwrap();
            delay.delay_ms(1000u32);
            led.toggle().unwrap();
            led2.toggle().unwrap();
        }
    }

    loop {}
}
