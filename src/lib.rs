//! # `defmt-bbq`
//!
//! > A generic [`bbqueue`] based transport for [`defmt`] log messages
//!
//! [`defmt`]: https://github.com/knurling-rs/defmt
//! [`bbqueue`]: https://github.com/jamesmunns/bbqueue
//!
//! `defmt` ("de format", short for "deferred formatting") is a highly efficient logging framework that targets resource-constrained devices, like microcontrollers.
//!
//! This crate stores the logged messages into a thread-safe FIFO queue, which can then be transferred
//! across any medium, such as USB, RS-485, or other transports.
//!
//! Although this crate acts as a `global_logger` implementer for `defmt`, it still requires
//! you to *do* something with the messages. This is intended to be a reusable building block
//! for a variety of different transport methods.
//!
//! ## Usage
//!
//! This crates requires it's users to perform the following actions:
//!
//! 1. (optional): If you'd like to select a different queue size than the default
//!   1024, you'll need to set the `DEFMT_BBQ_BUFFER_SIZE` environment variable at
//!   build time to configure the size. e.g.: `DEFMT_BBQ_BUFFER_SIZE=4096 cargo build`.
//! 2. Prior to the first `defmt` log, the user MUST call `defmt_bbq::init()`, which will
//!   initialize the logging buffer, and return the `Consumer` half of the queue, which
//!   gives access to incoming logging messages
//! 3. The user must regularly drain the logged messages. If the queue is filled, any
//!   additional bytes will be discarded, potentially corrupting (some) logging messages
//!
//! For more information on the Consumer interface, see the [Consumer docs] in the `bbqueue`
//! crate documentation.
//!
//! [Consumer docs]: https://docs.rs/bbqueue/latest/bbqueue/struct.Consumer.html
//!
//! ### Example
//!
//! ```rust,no_run
//! #[entry]
//! fn main() {
//!     // MUST be called before the first `defmt::*` call!
//!     let mut consumer = defmt_bbq::init().unwrap();
//!
//!     loop {
//!         defmt::println!("Hello, world!");
//!
//!         if let Some(grant) = consumer.read() {
//!             // do something with `bytes`, like send
//!             // it over a serial port..
//!
//!             // Then when done, make sure you release the grant
//!             // to free the space for future logging.
//!             let glen = grant.len();
//!             grant.release(glen);
//!         }
//!     }
//! }
//! ```
//!
//! ## Default Feature(s)
//!
//! This crate has a single default feature, which enables the `encoding-rzcobs`
//! feature of the `defmt` crate.
//!
//! It is **strongly recommended** to leave this feature enabled (and to
//! use rzcobs encoding) when using this crate. If the buffer is ever overfilled,
//! then the remaining bytes will be discarded, which will temporarily corrupt
//! the message stream.
//!
//! Because rzcobs is delimited by zero bytes, it is possible to recover
//! from this corruption, with the loss of a limited number of messages.
//! When using the "raw" encoding, you MUST ensure that the buffer is
//! **never** overfilled, as it will NOT be possible to recover from this
//! error condition.
//!
//! ## Provenance
//!
//! This repository is a fork of `defmt-rtt`, obtained from the [`defmt`] repository.
//!
//! This repository was forked as of upstream commit `50e3db37d5429ed3344726f01e1bc4bf04902251`.
//!
//! ## License
//!
//! Licensed under either of
//!
//! - Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
//!   http://www.apache.org/licenses/LICENSE-2.0)
//!
//! - MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)
//!
//! at your option.
//!
//! ### Contribution
//!
//! Unless you explicitly state otherwise, any contribution intentionally submitted
//! for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
//! licensed as above, without any additional terms or conditions.

#![no_std]

mod consts;

use core::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};
use cortex_m::{interrupt, register};
use bbqueue::{BBBuffer, Consumer, Producer, GrantW};

/// BBQueue buffer size. Default: 1024; can be customized by setting the
/// `DEFMT_RTT_BUFFER_SIZE` environment variable.
///
/// Use a power of 2 for best performance.
use crate::consts::BUF_SIZE;

struct UnsafeProducer {
    uc_mu_fp: UnsafeCell<MaybeUninit<Producer<'static, BUF_SIZE>>>,
}

impl UnsafeProducer {
    const fn new() -> Self {
        Self {
            uc_mu_fp: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    // TODO: Could be made safe if we ensure the reference is only taken
    // once. For now, leave unsafe
    unsafe fn get_mut(&self) -> &mut Producer<'static, BUF_SIZE> {
        assert_eq!(logstate::INIT_IDLE, BBQ_STATE.load(Ordering::Relaxed));

        // NOTE: `UnsafeCell` and `MaybeUninit` are both `#[repr(Transparent)],
        // meaning this direct cast is acceptable
        let const_ptr: *const Producer<'static, BUF_SIZE> = self.uc_mu_fp.get().cast();
        let mut_ptr: *mut Producer<'static, BUF_SIZE> = const_ptr as *mut _;
        let ref_mut: &mut Producer<'static, BUF_SIZE> = &mut *mut_ptr;
        ref_mut
    }
}

unsafe impl Sync for UnsafeProducer {}

struct UnsafeGrantW {
    uc_mu_fgw: UnsafeCell<MaybeUninit<GrantW<'static, BUF_SIZE>>>,
    offset: AtomicUsize,
}

impl UnsafeGrantW {
    const fn new() -> Self {
        Self {
            uc_mu_fgw: UnsafeCell::new(MaybeUninit::uninit()),
            offset: AtomicUsize::new(0),
        }
    }

    // TODO: Could be made safe if we ensure the reference is only taken
    // once. For now, leave unsafe.
    //
    // MUST be done in a critical section.
    unsafe fn put(&self, grant: GrantW<'static, BUF_SIZE>) {
        assert_eq!(logstate::INIT_IDLE, BBQ_STATE.load(Ordering::Relaxed));
        self.uc_mu_fgw.get().write(MaybeUninit::new(grant));
        BBQ_STATE.store(logstate::INIT_GRANT, Ordering::Relaxed);
    }

    unsafe fn take(&self) -> Option<GrantW<'static, BUF_SIZE>> {
        match BBQ_STATE.load(Ordering::Relaxed) {
            logstate::INIT_GRANT => {
                // NOTE: UnsafeCell and MaybeUninit are #[repr(Transparent)], so this
                // cast is acceptable
                let grant = self
                    .uc_mu_fgw
                    .get()
                    .cast::<GrantW<'static, BUF_SIZE>>()
                    .read();

                BBQ_STATE.store(logstate::INIT_IDLE, Ordering::Relaxed);

                Some(grant)
            }
            logstate::INIT_IDLE => {
                let producer = BBQ_PRODUCER.get_mut();
                self.offset.store(0, Ordering::Relaxed);
                producer
                    .grant_max_remaining(BUF_SIZE)
                    .ok()
            }
            _ => panic!("Internal Error!"),
        }
    }
}

unsafe impl Sync for UnsafeGrantW {}

// The underlying byte storage containing the logs. Always valid
static BBQ: BBBuffer<BUF_SIZE> = BBBuffer::new();

// A tracking variable for ensuring state. Always valid.
static BBQ_STATE: AtomicUsize = AtomicUsize::new(logstate::UNINIT);

// The producer half of the logging queue. This field is ONLY
// valid if `init()` has been called.
static BBQ_PRODUCER: UnsafeProducer = UnsafeProducer::new();

// An active write grant to a portion of the `BBQ`, obtained through
// the `BBQ_PRODUCER`. This field is ONLY valid if we are in the
// `INIT_GRANT` state.
static BBQ_GRANT_W: UnsafeGrantW = UnsafeGrantW::new();

mod logstate {
    // BBQ has NOT been initialized
    // BBQ_PRODUCER has NOT been initialized
    // BBQ_GRANT has NOT been initialized
    pub const UNINIT: usize = 0;

    // BBQ HAS been initialized
    // BBQ_PRODUCER HAS been initialized
    // BBQ_GRANT has NOT been initialized
    pub const INIT_IDLE: usize = 1;

    // BBQ HAS been initialized
    // BBQ_PRODUCER HAS been initialized
    // BBQ_GRANT HAS been initialized
    pub const INIT_GRANT: usize = 2;
}

/// Initialize the BBQueue based global defmt sink. MUST be called before
/// the first `defmt` log, or the log will panic!
///
/// On the first call to this function, the Consumer end of the logging
/// queue will be returned. On any subsequent call, an error will be
/// returned.
///
/// For more information on the Consumer interface, see the [Consumer docs] in the `bbqueue`
/// crate documentation.
///
/// [Consumer docs]: https://docs.rs/bbqueue/latest/bbqueue/struct.Consumer.html
pub fn init() -> Result<Consumer<'static, BUF_SIZE>, bbqueue::Error> {
    let (prod, cons) = BBQ.try_split()?;

    // NOTE: We are okay to treat the following as safe, as the BBQueue
    // split operation is guaranteed to only return Ok once in a
    // thread-safe manner.
    unsafe {
        BBQ_PRODUCER.uc_mu_fp.get().write(MaybeUninit::new(prod));
    }

    // MUST be done LAST
    BBQ_STATE.store(logstate::INIT_IDLE, Ordering::Release);

    Ok(cons)
}

#[defmt::global_logger]
struct Logger;

/// Global logger lock.
static TAKEN: AtomicBool = AtomicBool::new(false);
static INTERRUPTS_ACTIVE: AtomicBool = AtomicBool::new(false);
static mut ENCODER: defmt::Encoder = defmt::Encoder::new();

unsafe impl defmt::Logger for Logger {
    fn acquire() {
        let primask = register::primask::read();
        interrupt::disable();

        if TAKEN.load(Ordering::Relaxed) {
            panic!("defmt logger taken reentrantly")
        }

        // Ensure the logger has been initialized, and no grant is active
        assert_eq!(BBQ_STATE.load(Ordering::Relaxed), logstate::INIT_IDLE);

        // no need for CAS because interrupts are disabled
        TAKEN.store(true, Ordering::Relaxed);
        INTERRUPTS_ACTIVE.store(primask.is_active(), Ordering::Relaxed);

        // safety: accessing the `static mut` is OK because we have disabled interrupts.
        unsafe { ENCODER.start_frame(do_write) }
    }

    unsafe fn flush() {
        // We can't really do anything to flush, as the consumer is
        // in "userspace". Oh well.
    }

    unsafe fn release() {
        // safety: accessing the `static mut` is OK because we have disabled interrupts.
        ENCODER.end_frame(do_write);

        // If a grant is active, take it and commit it
        if let Some(grant) = BBQ_GRANT_W.take() {
            let offset = BBQ_GRANT_W.offset.load(Ordering::Relaxed);
            grant.commit(offset);
        }

        TAKEN.store(false, Ordering::Relaxed);
        if INTERRUPTS_ACTIVE.load(Ordering::Relaxed) {
            // re-enable interrupts
            interrupt::enable()
        }
    }

    unsafe fn write(bytes: &[u8]) {
        // safety: accessing the `static mut` is OK because we have disabled interrupts.
        ENCODER.write(bytes, do_write);
    }
}

// Drain as many bytes to the queue as possible. If the queue is filled,
// then any remaining bytes will be discarded.
fn do_write(mut remaining: &[u8]) {
    while !remaining.is_empty() {
        if let Some(mut grant) = unsafe { BBQ_GRANT_W.take() } {
            let mut offset = BBQ_GRANT_W.offset.load(Ordering::Relaxed);
            let glen = grant.len();

            let min = remaining.len().min(grant.len() - offset);
            grant[offset..][..min].copy_from_slice(&remaining[..min]);
            offset += min;

            remaining = &remaining[min..];

            if offset >= glen {
                grant.commit(offset);
            } else {
                unsafe { BBQ_GRANT_W.put(grant) }
            }
        } else {
            // Unable to obtain a grant. We're totally full. Sorry about
            // the rest of those bytes
            return;
        }
    }
}
