# Example

This folder contains an example of using `defmt-bbq` over the nRF52's USB CDC ACM Serial Port.

This example is designed to work on the nRF52840-DK, but could probably run on other nRF52840, or any usb-device implementations.

## Steps to run test:

1. Connect both USB ports of the nrf52840-dk
2. From one terminal, move to the `nrf52840-usb-serial` folder. Run this project with `cargo run --release`. You will not get logging output in this window
3. From another terminal, move to the `bbq-demo-print` folder. Start the program with `cargo run -- -e ../nrf52840-usb-serial/target/thumbv7em-none-eabihf/release/nrf52840-usb-serial`.

## Expected output

In the `nrf52840-usb-serial` terminal, you should see something like:

```
➜  nrf52840-usb-serial git:(main) ✗ cargo run --release
   Compiling nrf52840-usb-serial v0.1.0 (/home/james/personal/defmt-bbq/examples/nrf52840-usb-serial)
    Finished release [optimized + debuginfo] target(s) in 1.45s
     Running `probe-run --chip nRF52840_xxAA target/thumbv7em-none-eabihf/release/nrf52840-usb-serial`
(HOST) WARN  (BUG) location info is incomplete; it will be omitted from the output
(HOST) INFO  flashing program (5 pages / 20.00 KiB)
(HOST) INFO  success!
RTT logs not available; blocking until the device halts..
────────────────────────────────────────────────────────────────────────────────
```

In the `bbq-demo-print` terminal, you should see something like:

```
➜  bbq-demo-print git:(main) ✗ cargo run -- -e ../nrf52840-usb-serial/target/thumbv7em-none-eabihf/release/nrf52840-usb-serial
    Finished dev [unoptimized + debuginfo] target(s) in 0.02s
     Running `target/debug/bbq-demo-print -e ../nrf52840-usb-serial/target/thumbv7em-none-eabihf/release/nrf52840-usb-serial`
Ding! - 2000
Ding! - 3000
Ding! - 4000
Ding! - 5000
Ding! - 6000
Ding! - 7000
Ding! - 8000
Ding! - 9000
```
