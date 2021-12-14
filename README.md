# `defmt-bbq`

> A generic [`bbqueue`] based transport for [`defmt`] log messages

[`defmt`]: https://github.com/knurling-rs/defmt
[`bbqueue`]: https://github.com/jamesmunns/bbqueue

`defmt` ("de format", short for "deferred formatting") is a highly efficient logging framework that targets resource-constrained devices, like microcontrollers.

This crate stores the logged messages into a thread-safe FIFO queue, which can then be transferred
across any medium, such as USB, RS-485, or other transports.

Although this crate acts as a `global_logger` implementer for `defmt`, it still requires
you to *do* something with the messages. This is intended to be a reusable building block
for a variety of different transport methods.

## Usage

This crates requires it's users to perform the following actions:

1. (optional): If you'd like to select a different queue size than the default
  1024, you'll need to set the `DEFMT_BBQ_BUFFER_SIZE` environment variable at
  build time to configure the size. e.g.: `DEFMT_BBQ_BUFFER_SIZE=4096 cargo build`.
2. Prior to the first `defmt` log, the user MUST call `defmt_bbq::init()`, which will
  initialize the logging buffer, and return the `Consumer` half of the queue, which
  gives access to incoming logging messages
3. The user must regularly drain the logged messages. If the queue is filled, any
  additional bytes will be discarded, potentially corrupting (some) logging messages

For more information on the Consumer interface, see the [Consumer docs] in the `bbqueue`
crate documentation.

[Consumer docs]: https://docs.rs/bbqueue/latest/bbqueue/struct.Consumer.html

### Example

```rust
#[entry]
fn main() {
    // MUST be called before the first `defmt::*` call!
    let mut consumer = defmt_bbq::init().unwrap();

    loop {
        defmt::println!("Hello, world!");

        if let Some(grant) = consumer.read() {
            // do something with `bytes`, like send
            // it over a serial port..

            // Then when done, make sure you release the grant
            // to free the space for future logging.
            let glen = grant.len();
            grant.release(glen);
        }
    }
}
```

For a more detailed end-to-end example over USB Serial, please see the project's [example folder](./examples/README.md).

## Default Feature(s)

This crate has a single default feature, which enables the `encoding-rzcobs`
feature of the `defmt` crate.

It is **strongly recommended** to leave this feature enabled (and to
use rzcobs encoding) when using this crate. If the buffer is ever overfilled,
then the remaining bytes will be discarded, which will temporarily corrupt
the message stream.

Because rzcobs is delimited by zero bytes, it is possible to recover
from this corruption, with the loss of a limited number of messages.
When using the "raw" encoding, you MUST ensure that the buffer is
**never** overfilled, as it will NOT be possible to recover from this
error condition.

## Provenance

This repository is a fork of `defmt-rtt`, obtained from the [`defmt`] repository.

This repository was forked as of upstream commit `50e3db37d5429ed3344726f01e1bc4bf04902251`.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  http://www.apache.org/licenses/LICENSE-2.0)

- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
licensed as above, without any additional terms or conditions.
