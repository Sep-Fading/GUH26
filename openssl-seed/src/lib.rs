//! An OpenSSL 3 provider with one seed source, `MINICLAW-SYSTEM-SEED`, that reads the operating
//! system's random generator through `BCryptGenRandom`.
//!
//! The official Node build for Windows compiles its OpenSSL without `_WIN32_WINNT`, so OpenSSL's
//! own seed source calls the legacy `CryptAcquireContextW`, which reads provider registrations
//! from `HKLM\SOFTWARE\Microsoft\Cryptography` and is denied inside MiniClaw's sandbox. OpenSSL
//! lets a configuration file name another seed source (`seed` in the `random` section) and load
//! it from a provider module, so MiniClaw's private Node uses this one instead. It needs no
//! registry access and is the call OpenSSL itself makes when built for Windows Vista or later.
//!
//! Only the provider ABI is used (no OpenSSL library is linked): one exported entry point that
//! hands OpenSSL tables of plain C functions. Every pointer received from OpenSSL is checked
//! for null before use, and nothing here can unwind into OpenSSL.

// Raw C types and pointers of the provider ABI.
use std::ffi::{c_char, c_int, c_uint, c_void};
// Seed state shared between OpenSSL threads.
use std::sync::atomic::{AtomicI32, Ordering};

/// The algorithm name OpenSSL's configuration names in `seed = …`.
pub const SEED_NAME: &str = "MINICLAW-SYSTEM-SEED";

// Function and operation numbers from OpenSSL's `core_dispatch.h` (stable provider ABI).
const PROVIDER_TEARDOWN: c_int = 1024;
const PROVIDER_GETTABLE_PARAMS: c_int = 1025;
const PROVIDER_GET_PARAMS: c_int = 1026;
const PROVIDER_QUERY_OPERATION: c_int = 1027;
const OP_RAND: c_int = 5;
const RAND_NEWCTX: c_int = 1;
const RAND_FREECTX: c_int = 2;
const RAND_INSTANTIATE: c_int = 3;
const RAND_UNINSTANTIATE: c_int = 4;
const RAND_GENERATE: c_int = 5;
const RAND_RESEED: c_int = 6;
const RAND_ENABLE_LOCKING: c_int = 8;
const RAND_LOCK: c_int = 9;
const RAND_UNLOCK: c_int = 10;
const RAND_GETTABLE_CTX_PARAMS: c_int = 12;
const RAND_GET_CTX_PARAMS: c_int = 15;
const RAND_VERIFY_ZEROIZATION: c_int = 17;
const RAND_GET_SEED: c_int = 18;
const RAND_CLEAR_SEED: c_int = 19;

// `EVP_RAND_STATE_*` and `OSSL_PARAM` data types from OpenSSL's public headers.
const STATE_UNINITIALISED: i32 = 0;
const STATE_READY: i32 = 1;
const PARAM_INTEGER: c_uint = 1;
const PARAM_UNSIGNED_INTEGER: c_uint = 2;
const PARAM_UTF8_PTR: c_uint = 6;

/// The strength reported to OpenSSL in bits. A DRBG refuses a parent weaker than itself; the
/// strongest DRBG OpenSSL builds by default (CTR-DRBG with AES-256) asks for 256.
pub const STRENGTH: u32 = 256;
/// The largest request answered in one call, as OpenSSL's own seed source reports.
pub const MAX_REQUEST: usize = 128;

/// One entry of an OpenSSL function table.
#[repr(C)]
pub struct Dispatch {
    id: c_int,
    function: Option<unsafe extern "C" fn()>,
}

/// One algorithm offered for an operation.
#[repr(C)]
pub struct Algorithm {
    names: *const c_char,
    properties: *const c_char,
    implementation: *const Dispatch,
    description: *const c_char,
}

/// OpenSSL's typed parameter (`OSSL_PARAM`).
#[repr(C)]
pub struct Param {
    pub key: *const c_char,
    pub data_type: c_uint,
    pub data: *mut c_void,
    pub data_size: usize,
    pub return_size: usize,
}

/// Static tables hold raw pointers to static data only, so sharing them between threads is sound.
#[repr(transparent)]
struct Table<T>(T);
unsafe impl<T> Sync for Table<T> {}

/// Erases a function's signature for a dispatch entry, as OpenSSL's `(void (*)(void))` casts do.
macro_rules! entry {
    ($id:expr, $function:expr) => {
        Dispatch { id: $id, function: Some(unsafe { std::mem::transmute::<*const (), unsafe extern "C" fn()>($function as *const ()) }) }
    };
}

static RAND_FUNCTIONS: Table<[Dispatch; 15]> = Table([
    entry!(RAND_NEWCTX, seed_new),
    entry!(RAND_FREECTX, seed_free),
    entry!(RAND_INSTANTIATE, seed_instantiate),
    entry!(RAND_UNINSTANTIATE, seed_uninstantiate),
    entry!(RAND_GENERATE, seed_generate),
    entry!(RAND_RESEED, seed_reseed),
    entry!(RAND_ENABLE_LOCKING, seed_enable_locking),
    entry!(RAND_LOCK, seed_lock),
    entry!(RAND_UNLOCK, seed_unlock),
    entry!(RAND_GETTABLE_CTX_PARAMS, seed_gettable_ctx_params),
    entry!(RAND_GET_CTX_PARAMS, seed_get_ctx_params),
    entry!(RAND_VERIFY_ZEROIZATION, seed_verify_zeroization),
    entry!(RAND_GET_SEED, seed_get_seed),
    entry!(RAND_CLEAR_SEED, seed_clear_seed),
    Dispatch { id: 0, function: None },
]);

static ALGORITHMS: Table<[Algorithm; 2]> = Table([
    Algorithm { names: c"MINICLAW-SYSTEM-SEED".as_ptr(), properties: c"".as_ptr(), implementation: RAND_FUNCTIONS.0.as_ptr(),
        description: c"Seed source reading the Windows system random generator (BCryptGenRandom)".as_ptr() },
    Algorithm { names: std::ptr::null(), properties: std::ptr::null(), implementation: std::ptr::null(), description: std::ptr::null() },
]);

static PROVIDER_FUNCTIONS: Table<[Dispatch; 5]> = Table([
    entry!(PROVIDER_TEARDOWN, provider_teardown),
    entry!(PROVIDER_GETTABLE_PARAMS, provider_gettable_params),
    entry!(PROVIDER_GET_PARAMS, provider_get_params),
    entry!(PROVIDER_QUERY_OPERATION, provider_query_operation),
    Dispatch { id: 0, function: None },
]);

static GETTABLE: Table<[Param; 4]> = Table([
    Param { key: c"state".as_ptr(), data_type: PARAM_INTEGER, data: std::ptr::null_mut(), data_size: size_of::<c_int>(), return_size: usize::MAX },
    Param { key: c"strength".as_ptr(), data_type: PARAM_UNSIGNED_INTEGER, data: std::ptr::null_mut(), data_size: size_of::<c_uint>(), return_size: usize::MAX },
    Param { key: c"max_request".as_ptr(), data_type: PARAM_UNSIGNED_INTEGER, data: std::ptr::null_mut(), data_size: size_of::<usize>(), return_size: usize::MAX },
    Param { key: std::ptr::null(), data_type: 0, data: std::ptr::null_mut(), data_size: 0, return_size: 0 },
]);

static PROVIDER_GETTABLE: Table<[Param; 4]> = Table([
    Param { key: c"name".as_ptr(), data_type: PARAM_UTF8_PTR, data: std::ptr::null_mut(), data_size: 0, return_size: usize::MAX },
    Param { key: c"version".as_ptr(), data_type: PARAM_UTF8_PTR, data: std::ptr::null_mut(), data_size: 0, return_size: usize::MAX },
    Param { key: c"status".as_ptr(), data_type: PARAM_INTEGER, data: std::ptr::null_mut(), data_size: size_of::<c_int>(), return_size: usize::MAX },
    Param { key: std::ptr::null(), data_type: 0, data: std::ptr::null_mut(), data_size: 0, return_size: 0 },
]);

/// A provider context OpenSSL passes back unchanged; its address is all that matters.
static PROVIDER_CONTEXT: u8 = 0;

/// Fills a buffer from the system's preferred random generator. Returns false on any failure.
#[cfg(windows)]
pub fn fill(buffer: &mut [u8]) -> bool {
    #[link(name = "bcrypt")]
    unsafe extern "system" {
        fn BCryptGenRandom(algorithm: *mut c_void, buffer: *mut u8, length: u32, flags: u32) -> i32;
    }
    // BCRYPT_USE_SYSTEM_PREFERRED_RNG: no algorithm handle, no registry lookup.
    const SYSTEM_PREFERRED: u32 = 0x0000_0002;
    buffer.chunks_mut(u32::MAX as usize).all(|chunk| unsafe {
        BCryptGenRandom(std::ptr::null_mut(), chunk.as_mut_ptr(), chunk.len() as u32, SYSTEM_PREFERRED) == 0
    })
}

/// Fills a buffer from the kernel's random device, so the logic can be tested on other hosts.
#[cfg(not(windows))]
pub fn fill(buffer: &mut [u8]) -> bool {
    use std::io::Read;
    std::fs::File::open("/dev/urandom").and_then(|mut file| file.read_exact(buffer)).is_ok()
}

/// The provider's entry point. Hands OpenSSL the provider table and a context; always succeeds.
///
/// # Safety
/// Called by OpenSSL with valid out-pointers, as its provider ABI specifies.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn OSSL_provider_init(_handle: *const c_void, _core: *const Dispatch,
    out: *mut *const Dispatch, context: *mut *mut c_void) -> c_int {
    if out.is_null() || context.is_null() { return 0; }
    unsafe {
        *out = PROVIDER_FUNCTIONS.0.as_ptr();
        *context = (&PROVIDER_CONTEXT as *const u8).cast_mut().cast();
    }
    1
}

/// Releases nothing: the provider owns no state of its own.
unsafe extern "C" fn provider_teardown(_context: *mut c_void) {}

/// Lists the provider facts `provider_get_params` answers.
unsafe extern "C" fn provider_gettable_params(_context: *mut c_void) -> *const Param {
    PROVIDER_GETTABLE.0.as_ptr()
}

/// Answers the provider's name, version and status, as `openssl list -providers` asks.
unsafe extern "C" fn provider_get_params(_context: *mut c_void, params: *mut Param) -> c_int {
    each_param(params, |key, param| match key {
        b"name" => set_text(param, c"MiniClaw system seed provider"),
        b"version" => set_text(param, c"0.1.0"),
        b"status" => set_integer(param, 1),
        _ => true,
    })
}

/// Offers the one seed source for the random operation and nothing for any other operation.
unsafe extern "C" fn provider_query_operation(_context: *mut c_void, operation: c_int, no_cache: *mut c_int) -> *const Algorithm {
    if !no_cache.is_null() { unsafe { *no_cache = 0; } }
    if operation == OP_RAND { ALGORITHMS.0.as_ptr() } else { std::ptr::null() }
}

/// One seed source instance: only its life-cycle state, since every answer comes from Windows.
struct Seed {
    state: AtomicI32,
}

/// Borrows the seed behind an OpenSSL context pointer, or `None` for a null pointer.
fn seed<'a>(context: *mut c_void) -> Option<&'a Seed> {
    unsafe { context.cast::<Seed>().as_ref() }
}

/// Creates an instance. A seed source is the root of OpenSSL's chain, so a parent is refused.
unsafe extern "C" fn seed_new(_provider: *mut c_void, parent: *mut c_void, _parent_dispatch: *const Dispatch) -> *mut c_void {
    if !parent.is_null() { return std::ptr::null_mut(); }
    Box::into_raw(Box::new(Seed { state: AtomicI32::new(STATE_UNINITIALISED) })).cast()
}

/// Frees an instance made by `seed_new`.
unsafe extern "C" fn seed_free(context: *mut c_void) {
    if !context.is_null() { drop(unsafe { Box::from_raw(context.cast::<Seed>()) }); }
}

/// Marks the instance ready. Personalisation input is not needed by the system generator.
unsafe extern "C" fn seed_instantiate(context: *mut c_void, _strength: c_uint, _prediction: c_int,
    _personal: *const u8, _personal_len: usize, _params: *const Param) -> c_int {
    let Some(seed) = seed(context) else { return 0 };
    seed.state.store(STATE_READY, Ordering::SeqCst);
    1
}

/// Marks the instance uninstantiated.
unsafe extern "C" fn seed_uninstantiate(context: *mut c_void) -> c_int {
    let Some(seed) = seed(context) else { return 0 };
    seed.state.store(STATE_UNINITIALISED, Ordering::SeqCst);
    1
}

/// Fills the caller's buffer with system randomness. Fails when not ready or when Windows fails.
unsafe extern "C" fn seed_generate(context: *mut c_void, out: *mut u8, length: usize, strength: c_uint,
    _prediction: c_int, _additional: *const u8, _additional_len: usize) -> c_int {
    let Some(seed) = seed(context) else { return 0 };
    if seed.state.load(Ordering::SeqCst) != STATE_READY || strength > STRENGTH || (out.is_null() && length > 0) { return 0; }
    if length == 0 { return 1; }
    c_int::from(fill(unsafe { std::slice::from_raw_parts_mut(out, length) }))
}

/// Accepts a reseed request; the system generator reseeds itself.
unsafe extern "C" fn seed_reseed(context: *mut c_void, _prediction: c_int, _entropy: *const u8, _entropy_len: usize,
    _additional: *const u8, _additional_len: usize) -> c_int {
    seed(context).is_some_and(|seed| seed.state.load(Ordering::SeqCst) == STATE_READY).into()
}

/// Locking is accepted and costs nothing: the instance has no state to protect beyond an atomic.
unsafe extern "C" fn seed_enable_locking(_context: *mut c_void) -> c_int { 1 }

/// See `seed_enable_locking`.
unsafe extern "C" fn seed_lock(_context: *mut c_void) -> c_int { 1 }

/// See `seed_enable_locking`.
unsafe extern "C" fn seed_unlock(_context: *mut c_void) {}

/// Lists the parameters `seed_get_ctx_params` answers.
unsafe extern "C" fn seed_gettable_ctx_params(_context: *mut c_void, _provider: *mut c_void) -> *const Param {
    GETTABLE.0.as_ptr()
}

/// Nothing secret is kept, so there is nothing to zero.
unsafe extern "C" fn seed_verify_zeroization(_context: *mut c_void) -> c_int { 1 }

/// Writes an integer into a parameter of either integer type and of four or eight bytes.
fn set_integer(param: &mut Param, value: u64) -> bool {
    if param.data.is_null() || !matches!(param.data_type, PARAM_INTEGER | PARAM_UNSIGNED_INTEGER) { return false; }
    let signed = param.data_type == PARAM_INTEGER;
    match param.data_size {
        4 if value <= if signed { i32::MAX as u64 } else { u32::MAX as u64 } => unsafe { param.data.cast::<u32>().write_unaligned(value as u32) },
        8 if !signed || value <= i64::MAX as u64 => unsafe { param.data.cast::<u64>().write_unaligned(value) },
        _ => return false,
    }
    param.return_size = param.data_size;
    true
}

/// Points a text parameter at a static string, as `OSSL_PARAM_set_utf8_ptr` does.
fn set_text(param: &mut Param, text: &'static std::ffi::CStr) -> bool {
    if param.data.is_null() || param.data_type != PARAM_UTF8_PTR { return false; }
    unsafe { param.data.cast::<*const c_char>().write_unaligned(text.as_ptr()) };
    param.return_size = text.to_bytes().len();
    true
}

/// Calls `answer` for every parameter of a null-terminated array; stops at the first refusal.
fn each_param(params: *mut Param, mut answer: impl FnMut(&[u8], &mut Param) -> bool) -> c_int {
    if params.is_null() { return 1; }
    let mut index = 0;
    loop {
        let param = unsafe { &mut *params.add(index) };
        if param.key.is_null() { return 1; }
        let key = unsafe { std::ffi::CStr::from_ptr(param.key) }.to_bytes();
        if !answer(key, param) { return 0; }
        index += 1;
    }
}

/// Answers `state`, `strength` and `max_request`; other keys are left for OpenSSL's defaults.
///
/// # Safety
/// `params` is null or a valid array that ends with a null key, as OpenSSL passes it.
pub unsafe extern "C" fn seed_get_ctx_params(context: *mut c_void, params: *mut Param) -> c_int {
    let Some(seed) = seed(context) else { return 0 };
    each_param(params, |key, param| match key {
        b"state" => set_integer(param, seed.state.load(Ordering::SeqCst) as u64),
        b"strength" => set_integer(param, u64::from(STRENGTH)),
        b"max_request" => set_integer(param, MAX_REQUEST as u64),
        _ => true,
    })
}

/// The number of seed bytes for a request: enough for the entropy asked, within the given bounds.
pub fn seed_length(entropy_bits: c_int, min_len: usize, max_len: usize) -> Option<usize> {
    let wanted = (entropy_bits.max(0) as usize).div_ceil(8).max(min_len);
    (wanted <= max_len && wanted > 0).then_some(wanted)
}

/// Allocates and fills a seed buffer that `seed_clear_seed` later wipes and frees.
///
/// # Safety
/// `out` is a valid pointer to receive the buffer, as OpenSSL passes it.
pub unsafe extern "C" fn seed_get_seed(context: *mut c_void, out: *mut *mut u8, entropy: c_int, min_len: usize, max_len: usize,
    _prediction: c_int, _additional: *const u8, _additional_len: usize) -> usize {
    let Some(seed) = seed(context) else { return 0 };
    if out.is_null() || seed.state.load(Ordering::SeqCst) != STATE_READY { return 0; }
    let Some(length) = seed_length(entropy, min_len, max_len) else { return 0 };
    let mut buffer = vec![0u8; length].into_boxed_slice();
    if !fill(&mut buffer) { return 0; }
    unsafe { *out = Box::into_raw(buffer).cast::<u8>(); }
    length
}

/// Wipes and frees a buffer returned by `seed_get_seed`, given the length it returned.
///
/// # Safety
/// `buffer` and `length` are exactly a pointer and length that `seed_get_seed` returned.
pub unsafe extern "C" fn seed_clear_seed(_context: *mut c_void, buffer: *mut u8, length: usize) {
    if buffer.is_null() || length == 0 { return; }
    let mut owned = unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(buffer, length)) };
    for byte in owned.iter_mut() { unsafe { std::ptr::write_volatile(byte, 0) }; }
    drop(owned);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Finds one function of a dispatch table by number.
    fn function(table: &[Dispatch], id: c_int) -> Option<unsafe extern "C" fn()> {
        table.iter().take_while(|entry| entry.id != 0).find(|entry| entry.id == id).and_then(|entry| entry.function)
    }

    #[test]
    fn provider_offers_one_seed_source_for_random_only() {
        let mut out = std::ptr::null();
        let mut context = std::ptr::null_mut();
        assert_eq!(unsafe { OSSL_provider_init(std::ptr::null(), std::ptr::null(), &mut out, &mut context) }, 1);
        assert!(!context.is_null());
        let table = unsafe { std::slice::from_raw_parts(out, 5) };
        assert!(function(table, PROVIDER_QUERY_OPERATION).is_some() && function(table, PROVIDER_TEARDOWN).is_some());
        let mut no_cache = 1;
        let list = unsafe { provider_query_operation(context, OP_RAND, &mut no_cache) };
        assert_eq!(no_cache, 0);
        let first = unsafe { &*list };
        assert_eq!(unsafe { std::ffi::CStr::from_ptr(first.names) }.to_str().unwrap(), SEED_NAME);
        assert!(unsafe { (*list.add(1)).names.is_null() }, "exactly one algorithm");
        for other in [1, 2, 3, 4, 6, 10, 20] {
            assert!(unsafe { provider_query_operation(context, other, &mut no_cache) }.is_null(), "operation {other}");
        }
        assert_eq!(unsafe { OSSL_provider_init(std::ptr::null(), std::ptr::null(), std::ptr::null_mut(), &mut context) }, 0);
    }

    #[test]
    fn rand_table_is_complete_for_openssl() {
        // OpenSSL refuses a RAND implementation without the three generator functions, the
        // three context functions, and either both or neither of lock and unlock.
        let table = &RAND_FUNCTIONS.0;
        for id in [RAND_NEWCTX, RAND_FREECTX, RAND_GET_CTX_PARAMS, RAND_INSTANTIATE, RAND_UNINSTANTIATE, RAND_GENERATE,
            RAND_ENABLE_LOCKING, RAND_LOCK, RAND_UNLOCK, RAND_GET_SEED, RAND_CLEAR_SEED] {
            assert!(function(table, id).is_some(), "function {id}");
        }
        assert_eq!(table.last().unwrap().id, 0);
    }

    #[test]
    fn seed_source_refuses_a_parent_and_works_only_when_ready() {
        unsafe {
            assert!(seed_new(std::ptr::null_mut(), 1usize as *mut c_void, std::ptr::null()).is_null());
            let context = seed_new(std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null());
            let mut bytes = [0u8; 64];
            assert_eq!(seed_generate(context, bytes.as_mut_ptr(), bytes.len(), 256, 0, std::ptr::null(), 0), 0, "not instantiated");
            assert_eq!(seed_instantiate(context, 256, 0, std::ptr::null(), 0, std::ptr::null()), 1);
            assert_eq!(seed_generate(context, bytes.as_mut_ptr(), bytes.len(), 257, 0, std::ptr::null(), 0), 0, "stronger than offered");
            assert_eq!(seed_generate(context, bytes.as_mut_ptr(), bytes.len(), 256, 0, std::ptr::null(), 0), 1);
            let mut again = [0u8; 64];
            assert_eq!(seed_generate(context, again.as_mut_ptr(), again.len(), 128, 1, std::ptr::null(), 0), 1);
            assert_ne!(bytes, again);
            assert_ne!(bytes, [0u8; 64]);
            assert_eq!(seed_uninstantiate(context), 1);
            assert_eq!(seed_reseed(context, 0, std::ptr::null(), 0, std::ptr::null(), 0), 0);
            seed_free(context);
        }
    }

    #[test]
    fn parameters_report_state_strength_and_request_size() {
        unsafe {
            let context = seed_new(std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null());
            seed_instantiate(context, 0, 0, std::ptr::null(), 0, std::ptr::null());
            let (mut state, mut strength, mut request, mut other) = (-1i32, 0u32, 0usize, 7u32);
            let mut params = [
                Param { key: c"state".as_ptr(), data_type: PARAM_INTEGER, data: (&mut state as *mut i32).cast(), data_size: 4, return_size: usize::MAX },
                Param { key: c"strength".as_ptr(), data_type: PARAM_UNSIGNED_INTEGER, data: (&mut strength as *mut u32).cast(), data_size: 4, return_size: usize::MAX },
                Param { key: c"max_request".as_ptr(), data_type: PARAM_UNSIGNED_INTEGER, data: (&mut request as *mut usize).cast(), data_size: 8, return_size: usize::MAX },
                Param { key: c"reseed_counter".as_ptr(), data_type: PARAM_UNSIGNED_INTEGER, data: (&mut other as *mut u32).cast(), data_size: 4, return_size: usize::MAX },
                Param { key: std::ptr::null(), data_type: 0, data: std::ptr::null_mut(), data_size: 0, return_size: 0 },
            ];
            assert_eq!(seed_get_ctx_params(context, params.as_mut_ptr()), 1);
            assert_eq!((state, strength, request, other), (STATE_READY, STRENGTH, MAX_REQUEST, 7));
            assert_eq!(params[0].return_size, 4);
            assert_eq!(params[3].return_size, usize::MAX, "unknown keys are left alone");
            let mut short = 0u16;
            let mut bad = [Param { key: c"strength".as_ptr(), data_type: PARAM_UNSIGNED_INTEGER, data: (&mut short as *mut u16).cast(), data_size: 2, return_size: 0 },
                Param { key: std::ptr::null(), data_type: 0, data: std::ptr::null_mut(), data_size: 0, return_size: 0 }];
            assert_eq!(seed_get_ctx_params(context, bad.as_mut_ptr()), 0, "an unsupported size is an error, not a truncation");
            seed_free(context);
        }
    }

    #[test]
    fn seed_buffers_have_the_requested_size_and_are_returned() {
        assert_eq!(seed_length(256, 0, 128), Some(32));
        assert_eq!(seed_length(384, 48, 128), Some(48));
        assert_eq!(seed_length(256, 40, 128), Some(40));
        assert_eq!(seed_length(2048, 0, 128), None);
        assert_eq!(seed_length(-1, 0, 128), None);
        unsafe {
            let context = seed_new(std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null());
            let mut out = std::ptr::null_mut();
            assert_eq!(seed_get_seed(context, &mut out, 256, 0, 128, 0, std::ptr::null(), 0), 0, "not instantiated");
            seed_instantiate(context, 0, 0, std::ptr::null(), 0, std::ptr::null());
            let length = seed_get_seed(context, &mut out, 384, 16, 128, 0, std::ptr::null(), 0);
            assert_eq!(length, 48);
            assert!(!out.is_null());
            assert_ne!(std::slice::from_raw_parts(out, length), &[0u8; 48][..]);
            seed_clear_seed(context, out, length);
            seed_free(context);
        }
    }
}
