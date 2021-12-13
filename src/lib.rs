#![no_std]

mod consts;

use core::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};
use cortex_m::{interrupt, register};
use bbqueue::{BBBuffer, Consumer, Producer, GrantW};

/// BBQueue buffer size. Default: 1024; can be customized by setting the `DEFMT_RTT_BUFFER_SIZE` environment variable.
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

static BBQ: BBBuffer<BUF_SIZE> = BBBuffer::new();
static BBQ_STATE: AtomicUsize = AtomicUsize::new(logstate::UNINIT);

static BBQ_PRODUCER: UnsafeProducer = UnsafeProducer::new();
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

        if let Some(grant) = unsafe { BBQ_GRANT_W.take() } {
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

fn do_write(bytes: &[u8]) {
    let mut remaining = bytes;

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
            // Unable to obtain a grant. We're totally full
            return;
        }
    }
}
