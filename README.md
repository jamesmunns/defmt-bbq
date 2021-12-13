# `defmt-rtt`

> A generic [`bbqueue`] based transport for [`defmt`] log messages

[`defmt`]: https://github.com/knurling-rs/defmt
[`bbqueue`]: https://github.com/jamesmunns/bbqueue

`defmt` ("de format", short for "deferred formatting") is a highly efficient logging framework that targets resource-constrained devices, like microcontrollers.

This crate stores the logged messages into a thread-safe FIFO queue, which can then be transferred
across any medium, such as USB, RS-485, or other transports.

## Provenance

This repository is a hard-fork of `defmt-rtt`, obtained from the [`defmt`] repository.

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
