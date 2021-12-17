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
//! For a more detailed end-to-end example over USB Serial, please see the
//! project's [example folder].
//!
//! [example folder]: https://github.com/jamesmunns/defmt-bbq/blob/main/examples/README.md
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
//!   <http://www.apache.org/licenses/LICENSE-2.0>)
//!
//! - MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)
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

use bbqueue::{BBBuffer, GrantW, Producer};
use core::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, AtomicUsize, AtomicU8, Ordering},
};
use cortex_m::{interrupt, register};

#[derive(Debug, defmt::Format)]
pub enum Error {
    /// An internal latching fault has occured. No more logs will be returned.
    /// This indicates a coding error in `defmt-bbq`. Please open an issue.
    InternalLatchingFault,

    /// The user attempted to log before initializing the `defmt-bbq` structure.
    /// This is a latching fault, and not recoverable, but is not an indicator of
    /// a failure in the library. If you see this error, please ensure that you
    /// call `defmt-bbq::init()` before making any defmt logging statements.
    UseBeforeInitLatchingFault,

    /// This indicates some potentially recoverable bbqerror (including no
    /// data currently available).
    Bbq(BBQError),
}

impl From<BBQError> for Error {
    fn from(other: BBQError) -> Self {
        Error::Bbq(other)
    }
}

/// This is the consumer type given to the user to drain the logging queue.
///
/// It is a re-export of the [bbqueue::Consumer](https://docs.rs/bbqueue/latest/bbqueue/struct.Consumer.html) type.
///
pub use bbqueue::Consumer;

/// This is currently the only error type that may be returned.
///
/// It is a re-export of the [bbqueue::Error](https://docs.rs/bbqueue/latest/bbqueue/enum.Error.html) type.
///
pub use bbqueue::Error as BBQError;

pub use bbqueue::{GrantR, SplitGrantR};

/// A type alias of the defmt-bbq consumer.
///
/// This alias is useful for avoiding the lifetime and const generic bounds which
/// are always the same, if you need to name the consumer type
pub struct DefmtConsumer {
    cons: Consumer<'static, BUF_SIZE>,
}

impl DefmtConsumer {
    /// Obtains a contiguous slice of committed bytes. This slice may not
    /// contain ALL available bytes, if the writer has wrapped around. The
    /// remaining bytes will be available after all readable bytes are
    /// released
    pub fn read(&mut self) -> Result<GrantR<'static, BUF_SIZE>, Error> {
        Ok(self.cons.read()?)
    }

    /// Obtains two disjoint slices, which are each contiguous of committed bytes.
    /// Combined these contain all previously commited data.
    pub fn split_read(&mut self) -> Result<SplitGrantR<'static, BUF_SIZE>, Error> {
        Ok(self.cons.split_read()?)
    }
}

/// BBQueue buffer size. Default: 1024; can be customized by setting the
/// `DEFMT_RTT_BUFFER_SIZE` environment variable.
///
/// Use a power of 2 for best performance.
use crate::consts::BUF_SIZE;

/// A storage structure for holding the maybe initialized producer with inner mutability
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
    unsafe fn get_mut(&self) -> Result<&mut Producer<'static, BUF_SIZE>, Error> {
        latch_assert_eq(logstate::INIT_NO_STORED_GRANT, BBQ_STATE.load(Ordering::Relaxed))?;

        // NOTE: `UnsafeCell` and `MaybeUninit` are both `#[repr(Transparent)],
        // meaning this direct cast is acceptable
        let const_ptr: *const Producer<'static, BUF_SIZE> = self.uc_mu_fp.get().cast();
        let mut_ptr: *mut Producer<'static, BUF_SIZE> = const_ptr as *mut _;
        let ref_mut: &mut Producer<'static, BUF_SIZE> = &mut *mut_ptr;

        Ok(ref_mut)
    }
}

unsafe impl Sync for UnsafeProducer {}

struct UnsafeGrantW {
    uc_mu_fgw: UnsafeCell<MaybeUninit<GrantW<'static, BUF_SIZE>>>,

    /// Note: This stores the offset into the *current grant*, IFF a grant
    /// is stored in BBQ_GRANT_W. If there is no grant active, or if the
    /// grant is currently "taken" by the `do_write()` function, the value
    /// is meaningless.
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
    /// This function STORES
    /// MUST be done in a critical section.
    unsafe fn put(&self, grant: GrantW<'static, BUF_SIZE>, offset: usize) -> Result<(), Error> {
        // Note: This also catches the "already latched" state check
        latch_assert_eq(logstate::INIT_NO_STORED_GRANT, BBQ_STATE.load(Ordering::Relaxed))?;

        self.uc_mu_fgw.get().write(MaybeUninit::new(grant));
        self.offset.store(offset, Ordering::Relaxed);
        BBQ_STATE.store(logstate::INIT_GRANT_IS_STORED, Ordering::Relaxed);
        Ok(())
    }

    unsafe fn take(&self) -> Result<Option<(GrantW<'static, BUF_SIZE>, usize)>, Error> {
        check_latch(Ordering::Relaxed)?;

        Ok(match BBQ_STATE.load(Ordering::Relaxed) {
            logstate::INIT_GRANT_IS_STORED => {
                // NOTE: UnsafeCell and MaybeUninit are #[repr(Transparent)], so this
                // cast is acceptable
                let grant = self
                    .uc_mu_fgw
                    .get()
                    .cast::<GrantW<'static, BUF_SIZE>>()
                    .read();

                BBQ_STATE.store(logstate::INIT_NO_STORED_GRANT, Ordering::Relaxed);

                Some((grant, self.offset.load(Ordering::Relaxed)))
            }
            logstate::INIT_NO_STORED_GRANT => {
                let producer = BBQ_PRODUCER.get_mut()?;

                // We have a new grant, reset the current grant offset back to zero
                self.offset.store(0, Ordering::Relaxed);
                producer.grant_max_remaining(BUF_SIZE).ok().map(|g| (g, 0))
            }
            _ => {
                BBQ_STATE.store(logstate::LATCH_INTERNAL_ERROR, Ordering::Relaxed);
                return Err(Error::InternalLatchingFault);
            },
        })
    }
}

unsafe impl Sync for UnsafeGrantW {}

// The underlying byte storage containing the logs. Always valid
static BBQ: BBBuffer<BUF_SIZE> = BBBuffer::new();

// A tracking variable for ensuring state. Always valid.
static BBQ_STATE: AtomicU8 = AtomicU8::new(logstate::UNINIT);

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
    pub const UNINIT: u8 = 0;

    // BBQ HAS been initialized
    // BBQ_PRODUCER HAS been initialized
    // BBQ_GRANT has NOT been initialized
    pub const INIT_NO_STORED_GRANT: u8 = 1;

    // BBQ HAS been initialized
    // BBQ_PRODUCER HAS been initialized
    // BBQ_GRANT HAS been initialized
    pub const INIT_GRANT_IS_STORED: u8 = 2;

    // All state codes above 100 are a latching fault

    // A latching fault has occurred.
    pub const LATCH_INTERNAL_ERROR: u8 = 100;

    // The user attempted to log before init
    pub const LATCH_USE_BEFORE_INIT: u8 = 101;
}

#[inline]
fn check_latch(ordering: Ordering) -> Result<(), Error> {
    match BBQ_STATE.load(ordering) {
        i if i < logstate::LATCH_INTERNAL_ERROR => Ok(()),
        logstate::LATCH_USE_BEFORE_INIT => Err(Error::UseBeforeInitLatchingFault),
        _ => Err(Error::InternalLatchingFault),
    }
}

#[inline]
fn latch_assert_eq<T: PartialEq>(left: T, right: T) -> Result<(), Error> {
    if left == right {
        Ok(())
    } else {
        BBQ_STATE.store(logstate::LATCH_INTERNAL_ERROR, Ordering::Release);
        Err(Error::InternalLatchingFault)
    }
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
pub fn init() -> Result<DefmtConsumer, Error> {
    let (prod, cons) = BBQ.try_split()?;

    // NOTE: We are okay to treat the following as safe, as the BBQueue
    // split operation is guaranteed to only return Ok once in a
    // thread-safe manner.
    unsafe {
        BBQ_PRODUCER.uc_mu_fp.get().write(MaybeUninit::new(prod));
    }

    // MUST be done LAST
    BBQ_STATE.store(logstate::INIT_NO_STORED_GRANT, Ordering::Release);

    Ok(DefmtConsumer { cons })
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

        let state = BBQ_STATE.load(Ordering::Relaxed);
        let taken = TAKEN.load(Ordering::Relaxed);

        let bail = match (taken, state) {
            // Fast case: all good.
            (false, logstate::INIT_NO_STORED_GRANT) => false,

            // We tried to use before initialization. Regardless of the taken state,
            // this is an error. We *might* be able to recover from this in the future,
            // but it is more complicated. For now, just latch the error and signal
            // the user
            (_, logstate::UNINIT) => {
                BBQ_STATE.store(logstate::LATCH_USE_BEFORE_INIT, Ordering::Relaxed);
                true
            }

            // Either the taken flag is already set, or we are in an unexpected state
            // on acquisition. Either way, refuse to move forward.
            _ => {
                BBQ_STATE.store(logstate::LATCH_INTERNAL_ERROR, Ordering::Relaxed);
                true
            }
        };

        if bail {
            // If we just disabled interrupts, re-enable interrupts, then return
            if primask.is_active() {
                unsafe { interrupt::enable(); }
            }
            return;
        }

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
        // Don't return early, as we may need to re-enable interrupts.
        // `do_write` and `BBQ_GRANT_W.take()` will already early-return on
        // a latching fault condition

        // safety: accessing the `static mut` is OK because we have disabled interrupts.
        ENCODER.end_frame(do_write);

        // If a grant is active, take it and commit it
        match BBQ_GRANT_W.take() {
            Ok(Some((grant, offset))) => grant.commit(offset),

            // If we have no grant, or an internal error, keep going. We don't
            // want to early return, as that would prevent us from re-enabling
            // interrupts
            _ => {}
        }

        TAKEN.store(false, Ordering::Relaxed);
        if INTERRUPTS_ACTIVE.load(Ordering::Relaxed) {
            // re-enable interrupts
            interrupt::enable()
        }
    }

    unsafe fn write(bytes: &[u8]) {
        // Return early to avoid the encoder having to encode bytes we are going to throw away
        if check_latch(Ordering::Relaxed).is_err() {
            return;
        }

        // safety: accessing the `static mut` is OK because we have disabled interrupts.
        ENCODER.write(bytes, do_write);
    }
}

// Drain as many bytes to the queue as possible. If the queue is filled,
// then any remaining bytes will be discarded.
fn do_write(mut remaining: &[u8]) {
    while !remaining.is_empty() {
        match unsafe { BBQ_GRANT_W.take() } {
            Ok(Some((mut grant, mut offset))) => {
                let glen = grant.len();

                let min = remaining.len().min(grant.len() - offset);
                grant[offset..][..min].copy_from_slice(&remaining[..min]);
                offset += min;

                remaining = &remaining[min..];

                if offset >= glen {
                    grant.commit(offset);
                } else {
                    unsafe {
                        // If the put failed, return early
                        if BBQ_GRANT_W.put(grant, offset).is_err() {
                            return;
                        }
                    }
                }
            },

            // No grant available, just return. Bytes are dropped
            Ok(None) => return,

            // A latching fault is active. just return.
            Err(_) => return,
        }
    }
}
