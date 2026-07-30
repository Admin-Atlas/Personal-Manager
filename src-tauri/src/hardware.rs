// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Best-effort hardware scan for the local-AI Workbench (#296): how much memory, what CPU, how much
//! free disk, and — where we can read it — the GPU and its VRAM.
//!
//! `sysinfo` covers RAM / CPU / disk on every OS (it already calls the right native API under the
//! hood). It has no GPU/VRAM/battery, so those are hand-rolled per-OS: on Windows a CIM query for the
//! video controller, `nvidia-smi`, and a DXGI enumeration for a discrete card's true VRAM (the CIM
//! `AdapterRAM` field saturates at 4 GB); on Apple Silicon the unified-memory fraction; on Linux
//! `nvidia-smi`, the AMD sysfs node, and the Intel DRM memory-regions query for a discrete Arc
//! (#461). **No battery/AC here** — that's the deferred power-aware routing card (#432).
//!
//! The contract every probe honours: **a failure nulls its field, it never errors.** A machine with
//! no GPU, no `nvidia-smi`, or a driver that lies about VRAM still gets a complete, honest scan — the
//! missing pieces come back `None` with a plain note, never a hard error the UI has to handle.
//!
//! Units: everything is **GiB** (bytes / 2³⁰), labelled "GB" — which is how people read their RAM and
//! GPU specs (a "32 GB" stick is 32 GiB). The catalog's model sizes use the same base, so fit-scoring
//! compares like with like.

use serde::Serialize;

/// Bytes per GiB — the single unit conversion, so RAM/VRAM/disk all read in the base people expect.
const GIB: f64 = 1_073_741_824.0;

/// The GPU's `AdapterRAM` (a WMI `uint32`) saturates around 4 GiB, so any value at/above this ceiling
/// is a lie for a modern card — we fall back to a RAM-only score rather than trust it. (Only the
/// Windows probe reads `AdapterRAM`, so it's gated with its helper — see the pure-parse section note.)
#[cfg(any(windows, test))]
const ADAPTER_RAM_CEILING: u64 = 4_000_000_000;

/// The fraction of unified memory Apple Silicon lets the GPU use (`recommendedMaxWorkingSetSize` is
/// ~75% of total on current macOS). A rough but honest ceiling for the fit estimate. (Only compiled
/// where it's used — the macOS probe — plus test builds.)
#[cfg(any(target_os = "macos", test))]
const APPLE_VRAM_FRACTION: f64 = 0.75;

/// The machine, as far as local-model fit cares. Nullable fields are "couldn't read it", never zero.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Hardware {
    /// `windows` | `macos` | `linux` | other (`std::env::consts::OS`).
    pub platform: String,
    pub total_ram_gb: f64,
    pub available_ram_gb: f64,
    pub cpu_brand: Option<String>,
    pub cpu_cores: Option<u32>,
    pub cpu_threads: Option<u32>,
    /// Free space on the volume holding PM's data dir (where models download), when known.
    pub disk_free_gb: Option<f64>,
    pub gpu_name: Option<String>,
    pub gpu_vendor: Option<String>,
    pub vram_gb: Option<f64>,
    /// How VRAM was read: `nvidia-smi` | `dxgi` | `adapter_ram` | `apple_unified` | `amd_sysfs` |
    /// `drm_i915` | `drm_xe`.
    pub vram_source: Option<String>,
    /// The GPU's peak memory bandwidth (GB/s), matched from its name against a curated table, when
    /// recognised. `None` = an unlisted card (or a probe that only reports a generic name, e.g. Linux
    /// `nvidia-smi`): fit-scoring falls back to a flat default for the speed estimate. Sharpens the
    /// display-only tok/s only — never the fit verdict.
    pub gpu_bandwidth_gbps: Option<f64>,
    /// Apple-Silicon-style shared CPU/GPU memory (VRAM is a slice of system RAM, not separate).
    pub unified_memory: bool,
    pub is_wsl: bool,
    /// Honest, user-facing caveats gathered during the scan.
    pub notes: Vec<String>,
}

/// What a per-OS GPU probe found. Empty is a valid answer (no GPU, or nothing readable).
#[derive(Debug, Clone, Default)]
struct GpuProbe {
    name: Option<String>,
    vendor: Option<String>,
    vram_gb: Option<f64>,
    source: Option<String>,
    unified: bool,
    notes: Vec<String>,
}

/// Scan this machine. `data_dir` (PM's data folder) picks the disk whose free space matters for model
/// downloads; `None` falls back to the roomiest mounted volume. Blocking — call it off the async
/// runtime (the command wraps it in `spawn_blocking`).
pub fn scan(data_dir: Option<&std::path::Path>) -> Hardware {
    let mut hw = Hardware {
        platform: std::env::consts::OS.to_string(),
        ..Default::default()
    };

    fill_ram_cpu_disk(&mut hw, data_dir);

    let gpu = probe_gpu();
    hw.gpu_name = gpu.name;
    hw.gpu_vendor = gpu.vendor;
    hw.vram_gb = gpu.vram_gb;
    hw.vram_source = gpu.source;
    hw.unified_memory = gpu.unified;
    hw.notes.extend(gpu.notes);

    // Calibrate the GPU speed estimate to the actual card, when its name is specific enough to match
    // the curated bandwidth table (Windows reports full model names; a generic "NVIDIA GPU" from the
    // Linux probe won't match and falls back to the flat default — honest, never a wrong number). VRAM
    // disambiguates the few names that ship two bandwidths (RTX 3060/3080, Arc A770).
    hw.gpu_bandwidth_gbps = hw
        .gpu_name
        .as_deref()
        .and_then(|name| gpu_bandwidth_gbps(name, hw.vram_gb));

    hw
}

/// RAM / CPU / disk via `sysinfo` — one API, every OS. Each field is best-effort.
fn fill_ram_cpu_disk(hw: &mut Hardware, data_dir: Option<&std::path::Path>) {
    use sysinfo::System;

    let mut sys = System::new();
    sys.refresh_memory();
    sys.refresh_cpu_all();

    hw.total_ram_gb = round1(sys.total_memory() as f64 / GIB);
    hw.available_ram_gb = round1(sys.available_memory() as f64 / GIB);

    if let Some(cpu) = sys.cpus().first() {
        let brand = cpu.brand().trim();
        if !brand.is_empty() {
            hw.cpu_brand = Some(brand.to_string());
        }
    }
    hw.cpu_cores = System::physical_core_count().and_then(|c| u32::try_from(c).ok());
    let threads = sys.cpus().len();
    if threads > 0 {
        hw.cpu_threads = u32::try_from(threads).ok();
    }

    hw.disk_free_gb = disk_free_gb(data_dir);
}

/// Free GB on the volume holding `data_dir` (longest matching mount point), else the roomiest volume.
fn disk_free_gb(data_dir: Option<&std::path::Path>) -> Option<f64> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let list = disks.list();
    if list.is_empty() {
        return None;
    }

    if let Some(dir) = data_dir {
        let target = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        let best = list
            .iter()
            .filter(|d| target.starts_with(d.mount_point()))
            .max_by_key(|d| d.mount_point().as_os_str().len());
        if let Some(d) = best {
            return Some(round1(d.available_space() as f64 / GIB));
        }
    }

    list.iter()
        .map(|d| d.available_space())
        .max()
        .map(|b| round1(b as f64 / GIB))
}

/// Free BYTES on the volume holding `dir`, or `None` when no reported mount point contains it.
///
/// Deliberately not [`disk_free_gb`]: that one falls back to the roomiest volume, which is a
/// reasonable hint for the Workbench's "will this model fit" readout and a bad answer for a budget.
/// A caller sizing a write needs the volume it is about to write to or nothing at all — guessing
/// the wrong volume is worse than admitting we don't know.
pub(crate) fn available_bytes(dir: &std::path::Path) -> Option<u64> {
    let target = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let disks = sysinfo::Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .filter(|d| target.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space())
}

// --- GPU / VRAM: hand-rolled per OS ------------------------------------------------------------

#[cfg(windows)]
fn probe_gpu() -> GpuProbe {
    let mut probe = GpuProbe::default();

    // Video controller name + AdapterRAM via CIM (works without any vendor tool).
    if let Some(out) = run_capture(
        "powershell",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-CimInstance Win32_VideoController | Select-Object Name,AdapterRAM,DriverVersion | ConvertTo-Json -Compress",
        ],
    ) {
        if let Some(gpu) = pick_gpu(&parse_video_controller_json(&out)) {
            probe.vendor = vendor_from_name(&gpu.name);
            probe.name = Some(gpu.name);
            if let Some(bytes) = gpu.adapter_ram {
                if adapter_ram_reliable(bytes) {
                    probe.vram_gb = Some(round1(bytes as f64 / GIB));
                    probe.source = Some("adapter_ram".to_string());
                }
            }
        }
    }

    // nvidia-smi is authoritative for NVIDIA VRAM — prefer it over the AdapterRAM guess.
    if let Some(mib) =
        run_capture("nvidia-smi", &NVIDIA_SMI_ARGS).and_then(|s| parse_nvidia_smi_csv(&s))
    {
        probe.vram_gb = Some(round1(mib / 1024.0));
        probe.source = Some("nvidia-smi".to_string());
    }

    // Shared-memory (integrated) GPU → no distinct faster pool, so no GPU Split. nvidia-smi VRAM is
    // always a discrete NVIDIA card (even on a hybrid laptop), so it wins; otherwise classify by the
    // controller name (Intel UHD/Iris, AMD "Radeon Graphics" / Vega APU).
    probe.unified = probe.source.as_deref() != Some("nvidia-smi")
        && probe.name.as_deref().is_some_and(integrated_gpu_from_name);

    // Any discrete non-NVIDIA card over 4 GB (Intel Arc, AMD RX/Pro) saturates the uint32 AdapterRAM,
    // so we still have no VRAM — read the true DedicatedVideoMemory (a 64-bit SIZE_T) via DXGI. Only
    // for a discrete card (an integrated GPU has no distinct pool to size), and never overriding the
    // authoritative nvidia-smi figure (which already set vram_gb, so this is skipped).
    if probe.vram_gb.is_none() && !probe.unified {
        let adapters = dxgi_enumerate();
        if let Some(a) = pick_dxgi_discrete(&adapters) {
            probe.vram_gb = Some(round1(a.dedicated_bytes as f64 / GIB));
            probe.source = Some("dxgi".to_string());
            // DXGI can also name the card when CIM came back empty.
            if probe.name.is_none() {
                probe.name = Some(a.name.clone());
            }
            if probe.vendor.is_none() {
                probe.vendor = probe
                    .name
                    .as_deref()
                    .and_then(vendor_from_name)
                    .or_else(|| vendor_from_pci_id(a.vendor_id));
            }
        }
    }

    // A named GPU we couldn't size reliably: say so plainly, don't invent a number.
    if probe.name.is_some() && probe.vram_gb.is_none() {
        probe
            .notes
            .push("GPU VRAM couldn't be read reliably — sized on system RAM instead.".to_string());
    }
    probe
}

/// Enumerate the machine's DXGI adapters into our simple struct. Best-effort: any failure (no DXGI,
/// a driver quirk) yields an empty list and the caller sizes on system RAM instead. Needs **no** COM
/// apartment init — `CreateDXGIFactory1` is a direct `dxgi.dll` entry point — so it's safe to call
/// from this plain blocking scan on any thread.
#[cfg(windows)]
fn dxgi_enumerate() -> Vec<DxgiAdapter> {
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE,
    };
    let mut out = Vec::new();
    // SAFETY: factory creation and adapter enumeration are thread-agnostic and take no COM init. Each
    // COM interface (factory, adapter) is released on drop by the `windows` crate's RAII wrappers, and
    // `GetDesc1` fills a plain POD `DXGI_ADAPTER_DESC1` we copy out of immediately. `EnumAdapters1`
    // returns `Err(DXGI_ERROR_NOT_FOUND)` past the last adapter, ending the loop.
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else {
            return out;
        };
        let mut i = 0u32;
        while let Ok(adapter) = factory.EnumAdapters1(i) {
            i += 1;
            let Ok(desc) = adapter.GetDesc1() else {
                continue;
            };
            out.push(DxgiAdapter {
                name: utf16_trim_to_string(&desc.Description),
                vendor_id: desc.VendorId,
                dedicated_bytes: desc.DedicatedVideoMemory as u64,
                is_software: (desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0,
            });
        }
    }
    out
}

#[cfg(target_os = "macos")]
fn probe_gpu() -> GpuProbe {
    let mut probe = GpuProbe::default();
    if cfg!(target_arch = "aarch64") {
        // Apple Silicon: the GPU shares system RAM. Size VRAM as the working-set fraction of total.
        let total_gb = {
            let mut sys = sysinfo::System::new();
            sys.refresh_memory();
            sys.total_memory() as f64 / GIB
        };
        probe.name = Some("Apple Silicon GPU".to_string());
        probe.vendor = Some("Apple".to_string());
        probe.vram_gb = Some(round1(apple_vram_gb(total_gb)));
        probe.source = Some("apple_unified".to_string());
        probe.unified = true;
    } else {
        probe
            .notes
            .push("GPU VRAM isn't read on Intel Macs — sized on system RAM instead.".to_string());
    }
    probe
}

#[cfg(target_os = "linux")]
fn probe_gpu() -> GpuProbe {
    let mut probe = GpuProbe::default();

    if is_wsl_now() {
        probe
            .notes
            .push("Running under WSL — GPU passthrough may vary.".to_string());
    }

    // NVIDIA first (authoritative), then the AMD sysfs node, then the Intel DRM query (#461).
    if let Some(mib) =
        run_capture("nvidia-smi", &NVIDIA_SMI_ARGS).and_then(|s| parse_nvidia_smi_csv(&s))
    {
        probe.name = Some("NVIDIA GPU".to_string());
        probe.vendor = Some("NVIDIA".to_string());
        probe.vram_gb = Some(round1(mib / 1024.0));
        probe.source = Some("nvidia-smi".to_string());
    } else if let Some(read) = pick_linux_non_nvidia_gpu() {
        probe.name = Some(format!("{} GPU", read.vendor));
        probe.vendor = Some(read.vendor.to_string());
        probe.vram_gb = Some(round1(read.bytes as f64 / GIB));
        probe.source = Some(read.source.to_string());
        // Integrated → the "VRAM" is a shared-RAM carve-out, not a distinct faster pool: no Split.
        probe.unified = read.unified;
    }
    probe
}

/// A non-NVIDIA GPU memory reading on Linux, before it's folded into the probe.
#[cfg(target_os = "linux")]
struct LinuxVramReading {
    bytes: u64,
    /// `AMD` | `Intel` — also the display name's prefix.
    vendor: &'static str,
    /// The `vram_source` label: `amd_sysfs` | `drm_i915` | `drm_xe`.
    source: &'static str,
    /// Shared-memory (integrated) part, so there's no distinct faster pool to offer.
    unified: bool,
}

/// The AMD sysfs node and the Intel DRM query, resolved to whichever describes a **discrete** card.
/// Both are read because the two can coexist: an AMD APU always publishes a `mem_info_vram_total`
/// carve-out, so on a Ryzen-APU box with an Arc card installed, taking the first answer would report
/// the shared-RAM slice and miss the card that actually has a distinct, faster pool. An Intel reading
/// is discrete by construction (see [`read_intel_drm_vram`]), so it wins over an integrated AMD one
/// and never over a discrete one.
#[cfg(target_os = "linux")]
fn pick_linux_non_nvidia_gpu() -> Option<LinuxVramReading> {
    let amd = read_amd_sysfs_vram().map(|(bytes, card)| LinuxVramReading {
        bytes,
        vendor: "AMD",
        source: "amd_sysfs",
        unified: amd_is_integrated(card),
    });
    match amd {
        Some(a) if !a.unified => Some(a),
        Some(a) => read_intel_drm_vram().or(Some(a)),
        None => read_intel_drm_vram(),
    }
}

// Fallback for any other target: no GPU probe.
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn probe_gpu() -> GpuProbe {
    GpuProbe::default()
}

// nvidia-smi is queried by the Windows and Linux probes; macOS never shells out to it.
#[cfg(any(windows, target_os = "linux"))]
const NVIDIA_SMI_ARGS: [&str; 2] = ["--query-gpu=memory.total", "--format=csv,noheader,nounits"];

/// Run a command and capture stdout as a string, or `None` if it can't be run or fails. On Windows a
/// no-op-elsewhere `no_window` flag keeps a console from flashing. Only the Windows/Linux probes shell
/// out (macOS reads memory directly), so this is gated to them.
#[cfg(any(windows, target_os = "linux"))]
fn run_capture(program: &str, args: &[&str]) -> Option<String> {
    let mut cmd = std::process::Command::new(program);
    cmd.args(args);
    no_window(&mut cmd);
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Hide the console window a spawned CLI would otherwise flash on Windows. No-op elsewhere. (Mirrors
/// the one-liner in `sidecar.rs` — kept local to avoid a shared-module dependency for one constant.)
#[cfg(windows)]
fn no_window(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}
// The no-op arm only needs to exist where `run_capture` is compiled but isn't Windows — i.e. Linux.
#[cfg(target_os = "linux")]
fn no_window(_cmd: &mut std::process::Command) {}

#[cfg(target_os = "linux")]
fn read_amd_sysfs_vram() -> Option<(u64, u32)> {
    // The first card's total VRAM in bytes AND its index, if the amdgpu driver exposes it. The index
    // lets `amd_is_integrated` check the SAME card's PCI bus (APU-vs-discrete).
    for n in 0..8 {
        let path = format!("/sys/class/drm/card{n}/device/mem_info_vram_total");
        if let Ok(s) = std::fs::read_to_string(&path) {
            if let Ok(bytes) = s.trim().parse::<u64>() {
                if bytes > 0 {
                    return Some((bytes, n));
                }
            }
        }
    }
    None
}

/// Whether amdgpu `card{n}` is integrated (an APU): its PCI device sits on bus 00 (the CPU root
/// complex), whereas a discrete card is behind a PCIe port on a higher bus. Best-effort — if the
/// `device` symlink can't be read it falls back to `false` (discrete), but since we only ask about a
/// card whose `.../device/mem_info_vram_total` we just read, that link is present in practice.
#[cfg(target_os = "linux")]
fn amd_is_integrated(card: u32) -> bool {
    std::fs::read_link(format!("/sys/class/drm/card{card}/device"))
        .ok()
        .and_then(|p| p.to_str().map(str::to_string))
        .is_some_and(|target| pci_bus_is_integrated(&target))
}

#[cfg(target_os = "linux")]
fn is_wsl_now() -> bool {
    std::fs::read_to_string("/proc/version")
        .map(|v| detect_wsl(&v))
        .unwrap_or(false)
}

// --- Linux: discrete Intel VRAM via the DRM query ioctl (#461) ----------------------------------
//
// Mainline exposes no VRAM-size sysfs node for Intel — `mem_info_vram_total` is amdgpu-only — so a
// discrete Arc's memory can only be read through the driver's query ioctl. The two Intel drivers
// answer the same question with a different ioctl number and a different blob layout, so the node's
// driver name picks the path:
//   * **i915** — `DRM_IOCTL_I915_QUERY` / `DRM_I915_QUERY_MEMORY_REGIONS`, summing `probed_size` over
//     regions whose class is `I915_MEMORY_CLASS_DEVICE` (1).
//   * **xe** — `DRM_IOCTL_XE_DEVICE_QUERY` / `DRM_XE_DEVICE_QUERY_MEM_REGIONS`, summing `total_size`
//     over regions whose class is `DRM_XE_MEM_REGION_CLASS_VRAM` (1).
// Both queries are `DRM_RENDER_ALLOW`, so an `O_RDONLY` open of a render node is enough: no root, no
// DRM master, no chance of disturbing the display. Render nodes are also world-readable on virtually
// every distro, where `card*` needs the `video` group.
//
// **An integrated Intel GPU reports no device-local region at all** — only system memory — which is
// precisely the discrete gate we want. No class-1 region ⇒ `None` ⇒ the scan sizes on system RAM
// exactly as it did before, so this can only ever add a reading, never change an existing one.
//
// HONEST LIMIT: the parsers and the ioctl-number encoding below are unit-tested on every platform,
// but the syscall path itself has never run on a discrete-Arc-on-Linux machine — there is no such
// machine here and no CI runner has one. Every failure nulls the reading, so an untested box degrades
// to the pre-#461 behaviour rather than reporting something wrong.

/// The Intel DRM driver bound to a node. They share no ABI, so this picks the query path.
#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IntelDrmDriver {
    I915,
    Xe,
}

#[cfg(any(target_os = "linux", test))]
impl IntelDrmDriver {
    /// The `vram_source` label a reading from this driver carries.
    fn source_label(self) -> &'static str {
        match self {
            IntelDrmDriver::I915 => "drm_i915",
            IntelDrmDriver::Xe => "drm_xe",
        }
    }
}

/// Dedicated VRAM on a discrete Intel card, by querying every Intel render node and taking the
/// largest device-local total. Enumerating rather than assuming `renderD128` matters on a hybrid
/// laptop, where 128 is routinely the integrated GPU and the discrete card is 129.
#[cfg(target_os = "linux")]
fn read_intel_drm_vram() -> Option<LinuxVramReading> {
    let mut best: Option<(u64, IntelDrmDriver)> = None;
    for entry in std::fs::read_dir("/sys/class/drm").ok()?.flatten() {
        let node = entry.file_name();
        let Some(node) = node.to_str() else { continue };
        if !node.starts_with("renderD") {
            continue;
        }
        let device = format!("/sys/class/drm/{node}/device");
        // PCI vendor 0x8086 = Intel. Anything else is another vendor's render node.
        if !std::fs::read_to_string(format!("{device}/vendor"))
            .is_ok_and(|v| is_intel_pci_vendor(&v))
        {
            continue;
        }
        let Some(driver) = std::fs::read_link(format!("{device}/driver"))
            .ok()
            .and_then(|p| p.to_str().and_then(intel_drm_driver_from_link))
        else {
            continue;
        };
        if let Some(bytes) = query_intel_vram(&format!("/dev/dri/{node}"), driver) {
            if best.is_none_or(|(b, _)| bytes > b) {
                best = Some((bytes, driver));
            }
        }
    }
    best.map(|(bytes, driver)| LinuxVramReading {
        bytes,
        vendor: "Intel",
        source: driver.source_label(),
        // A device-local region only exists on a discrete card, so a reading is never unified.
        unified: false,
    })
}

/// Open one render node read-only and run the driver's memory-regions query. Best-effort: any
/// failure — a permission denial, an older kernel without the query, a driver that answers `ENOTTY` —
/// yields `None`.
#[cfg(target_os = "linux")]
fn query_intel_vram(node_path: &str, driver: IntelDrmDriver) -> Option<u64> {
    let path = std::ffi::CString::new(node_path).ok()?;
    // SAFETY: `path` is a valid NUL-terminated C string that outlives the call. `open` returns -1 on
    // failure and never takes ownership of the pointer. O_EXCL is deliberately NOT passed — the DRM
    // open path rejects it outright.
    let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
    if fd < 0 {
        return None;
    }
    let guard = OwnedFd(fd);
    let blob = match driver {
        IntelDrmDriver::I915 => i915_query_memory_regions(guard.0),
        IntelDrmDriver::Xe => xe_query_mem_regions(guard.0),
    }?;
    let bytes = match driver {
        IntelDrmDriver::I915 => parse_i915_vram_bytes(&blob),
        IntelDrmDriver::Xe => parse_xe_vram_bytes(&blob),
    }?;
    (bytes > 0).then_some(bytes)
}

/// Closes its file descriptor on drop, so every early return from the query path releases the node.
#[cfg(target_os = "linux")]
struct OwnedFd(libc::c_int);

#[cfg(target_os = "linux")]
impl Drop for OwnedFd {
    fn drop(&mut self) {
        // SAFETY: `self.0` came from a successful `open` and is closed exactly once, here.
        unsafe { libc::close(self.0) };
    }
}

/// Run the i915 two-pass memory-regions query, returning the raw reply blob.
///
/// Pass 1 sets `length = 0`, which asks the kernel for the byte count and leaves `data_ptr` alone;
/// the count comes back **in the item**, not as the ioctl's return value. Pass 2 replays the same
/// item with `data_ptr` pointing at a zeroed buffer of exactly that size.
///
/// Two details are load-bearing and easy to get wrong. `length` is **signed**: a per-item failure is
/// reported as a negative `-errno` while the ioctl itself still returns 0, so an unsigned read of a
/// `-EINVAL` from a pre-5.14 kernel would turn into a 4 GiB allocation. And the buffer must be
/// **zeroed**: the kernel reads the 16-byte header back out of it and rejects the whole query if the
/// must-be-zero fields aren't.
#[cfg(target_os = "linux")]
fn i915_query_memory_regions(fd: libc::c_int) -> Option<Vec<u8>> {
    let mut item = DrmI915QueryItem {
        query_id: u64::from(DRM_I915_QUERY_MEMORY_REGIONS),
        ..Default::default()
    };
    let mut query = DrmI915Query {
        num_items: 1,
        flags: 0,
        items_ptr: std::ptr::addr_of_mut!(item) as u64,
    };

    // SAFETY: `fd` is an open DRM render node. The request word is built from `size_of` of the very
    // struct we pass, so the kernel's `_IOC_SIZE` bound matches our allocation exactly. `items_ptr`
    // points at a live, correctly-sized `DrmI915QueryItem` that outlives the call.
    if unsafe { libc::ioctl(fd, DRM_IOCTL_I915_QUERY as _, std::ptr::addr_of_mut!(query)) } < 0 {
        return None;
    }
    let len = i915_blob_len(item.length)?;

    // `Vec<u64>` for the allocation so the buffer is 8-aligned as the ABI expects (both blob layouts
    // are whole multiples of 8), and zeroed so the header's must-be-zero fields pass validation.
    let mut buf = vec![0u64; len / 8];
    item.data_ptr = buf.as_mut_ptr() as u64;
    query.items_ptr = std::ptr::addr_of_mut!(item) as u64;

    // SAFETY: as above; `data_ptr` now points at `len` zeroed, 8-aligned bytes we own, and `length`
    // still holds the exact count the kernel asked for.
    if unsafe { libc::ioctl(fd, DRM_IOCTL_I915_QUERY as _, std::ptr::addr_of_mut!(query)) } < 0 {
        return None;
    }
    // Re-read the length: the kernel revises it down if we over-asked.
    let filled = i915_blob_len(item.length).unwrap_or(len).min(len);
    Some(words_to_bytes(&buf, filled))
}

/// Run the xe two-pass memory-regions query, returning the raw reply blob. Pass 1 sets `size = 0` and
/// the kernel writes the required byte count back into that same field; pass 2 must replay it
/// **unchanged** (any other non-zero size is rejected) with `data` pointing at the buffer.
#[cfg(target_os = "linux")]
fn xe_query_mem_regions(fd: libc::c_int) -> Option<Vec<u8>> {
    let mut query = DrmXeDeviceQuery {
        query: DRM_XE_DEVICE_QUERY_MEM_REGIONS,
        ..Default::default()
    };

    // SAFETY: `fd` is an open DRM render node; the request word encodes `size_of::<DrmXeDeviceQuery>()`,
    // matching the struct we hand over. `extensions` and `reserved` are zero, which the driver requires.
    if unsafe {
        libc::ioctl(
            fd,
            DRM_IOCTL_XE_DEVICE_QUERY as _,
            std::ptr::addr_of_mut!(query),
        )
    } < 0
    {
        return None;
    }
    let len = xe_blob_len(query.size)?;

    let mut buf = vec![0u64; len / 8];
    query.data = buf.as_mut_ptr() as u64;

    // SAFETY: as above; `data` points at `len` zeroed, 8-aligned bytes we own, and `size` is exactly
    // the value the kernel wrote back on the probe pass.
    if unsafe {
        libc::ioctl(
            fd,
            DRM_IOCTL_XE_DEVICE_QUERY as _,
            std::ptr::addr_of_mut!(query),
        )
    } < 0
    {
        return None;
    }
    Some(words_to_bytes(&buf, len))
}

/// Reinterpret the `u64` reply buffer as its first `len` bytes, in native order — the encoding the
/// kernel wrote. A plain copy of a few hundred bytes, which keeps the parsers safe `&[u8]` functions
/// testable on every platform.
#[cfg(target_os = "linux")]
fn words_to_bytes(buf: &[u64], len: usize) -> Vec<u8> {
    buf.iter().flat_map(|w| w.to_ne_bytes()).take(len).collect()
}

// --- pure parse helpers (unit-tested; the live probes above are not) -----------------------------
//
// Each is `#[cfg]`-gated to the OS whose probe calls it, plus `test` so every platform's test run
// still exercises the parser. Without the gate, clippy's `-D warnings` dead-code lint fails the build
// on the platforms that don't use a given helper — a Windows `AdapterRAM` parser is dead on Linux.

#[cfg(any(windows, test))]
#[derive(Debug, Clone)]
struct GpuLine {
    name: String,
    adapter_ram: Option<u64>,
}

/// Parse `Get-CimInstance Win32_VideoController | ConvertTo-Json` — which is a single object for one
/// GPU and an array for several. Missing/zero `AdapterRAM` becomes `None`.
#[cfg(any(windows, test))]
fn parse_video_controller_json(json: &str) -> Vec<GpuLine> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json.trim()) else {
        return Vec::new();
    };
    let objects: Vec<&serde_json::Value> = match &value {
        serde_json::Value::Array(a) => a.iter().collect(),
        serde_json::Value::Object(_) => vec![&value],
        _ => return Vec::new(),
    };
    objects
        .into_iter()
        .filter_map(|o| {
            let name = o.get("Name")?.as_str()?.trim().to_string();
            if name.is_empty() {
                return None;
            }
            // AdapterRAM can arrive as a JSON number or a stringified number; 0/negative → None.
            let adapter_ram = o.get("AdapterRAM").and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.trim().parse::<u64>().ok()))
            });
            let adapter_ram = adapter_ram.filter(|&b| b > 0);
            Some(GpuLine { name, adapter_ram })
        })
        .collect()
}

/// Choose the most relevant controller: skip Microsoft's basic/remote display shims, then take the
/// one reporting the most memory (usually the discrete card), else the first real one.
#[cfg(any(windows, test))]
fn pick_gpu(lines: &[GpuLine]) -> Option<GpuLine> {
    let real: Vec<&GpuLine> = lines
        .iter()
        .filter(|l| {
            let n = l.name.to_ascii_lowercase();
            !n.contains("basic display") && !n.contains("remote display")
        })
        .collect();
    let pool = if real.is_empty() {
        lines.iter().collect::<Vec<_>>()
    } else {
        real
    };
    pool.into_iter()
        .max_by_key(|l| l.adapter_ram.unwrap_or(0))
        .cloned()
}

/// `AdapterRAM` is a `uint32` that saturates near 4 GiB, so only sub-ceiling values are trustworthy.
#[cfg(any(windows, test))]
fn adapter_ram_reliable(bytes: u64) -> bool {
    bytes > 0 && bytes < ADAPTER_RAM_CEILING
}

/// A DXGI adapter as we care about it: its description, PCI vendor id, dedicated-VRAM bytes (a 64-bit
/// `SIZE_T`, so no `AdapterRAM` saturation), and whether it's the software/WARP renderer (which has no
/// real VRAM and must be ignored). The live enumerator fills these from `DXGI_ADAPTER_DESC1`; the pure
/// picker below is what the tests exercise.
#[cfg(any(windows, test))]
#[derive(Debug, Clone)]
struct DxgiAdapter {
    name: String,
    vendor_id: u32,
    dedicated_bytes: u64,
    is_software: bool,
}

/// A dedicated pool below this isn't a discrete card's VRAM — it's a BIOS-reserved integrated carve-out
/// (typically 128–512 MiB) or a stub. Discrete GPUs relevant to local models start at a couple of GiB,
/// so a 1 GiB floor cleanly separates them and keeps an unrecognised iGPU from reporting a "VRAM" number.
#[cfg(any(windows, test))]
const DXGI_MIN_DEDICATED_BYTES: u64 = 1_073_741_824;

/// The hardware adapter with the most dedicated VRAM, if it clears the discrete floor — i.e. the
/// discrete card. Skips the software/WARP renderer. `None` on an integrated-only machine (no adapter
/// clears the floor), which is correct: there's no distinct VRAM pool to size a GPU-resident config in.
#[cfg(any(windows, test))]
fn pick_dxgi_discrete(adapters: &[DxgiAdapter]) -> Option<&DxgiAdapter> {
    adapters
        .iter()
        .filter(|a| !a.is_software && a.dedicated_bytes >= DXGI_MIN_DEDICATED_BYTES)
        .max_by_key(|a| a.dedicated_bytes)
}

/// A GPU vendor from its PCI vendor id — the last-resort fill when DXGI is the only source that named
/// the card (CIM returned nothing) and the description string didn't classify. `None` for anything
/// outside the three GPU vendors (e.g. `0x1414`, Microsoft's WARP — already filtered as software).
#[cfg(any(windows, test))]
fn vendor_from_pci_id(vendor_id: u32) -> Option<String> {
    match vendor_id {
        0x10DE => Some("NVIDIA".to_string()),
        0x1002 => Some("AMD".to_string()),
        0x8086 => Some("Intel".to_string()),
        _ => None,
    }
}

/// Decode a fixed-size, NUL-padded UTF-16 buffer (a `DXGI_ADAPTER_DESC1.Description` is `[u16; 128]`)
/// into a trimmed `String`, stopping at the first NUL. Lossy on the rare invalid code unit.
#[cfg(any(windows, test))]
fn utf16_trim_to_string(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end]).trim().to_string()
}

/// The largest `memory.total` (MiB) across `nvidia-smi --query-gpu` lines, or `None`.
#[cfg(any(windows, target_os = "linux", test))]
fn parse_nvidia_smi_csv(csv: &str) -> Option<f64> {
    csv.lines()
        .filter_map(|l| l.trim().parse::<f64>().ok())
        .filter(|&m| m > 0.0)
        .fold(None, |acc: Option<f64>, m| {
            Some(acc.map_or(m, |a| a.max(m)))
        })
}

/// A GPU vendor guessed from the controller name, or `None` if unrecognized.
#[cfg(any(windows, test))]
fn vendor_from_name(name: &str) -> Option<String> {
    let n = name.to_ascii_lowercase();
    if n.contains("nvidia")
        || n.contains("geforce")
        || n.contains("quadro")
        || n.contains("rtx")
        || n.contains("gtx")
    {
        Some("NVIDIA".to_string())
    } else if n.contains("amd") || n.contains("radeon") || n.contains("ati ") {
        Some("AMD".to_string())
    } else if n.contains("intel") || n.contains("arc ") || n.contains("uhd") || n.contains("iris") {
        Some("Intel".to_string())
    } else if n.contains("apple") {
        Some("Apple".to_string())
    } else {
        None
    }
}

/// Whether a video-controller name is an integrated GPU (shares system memory, so no faster "GPU"
/// config to offer). Intel iGPUs are UHD / Iris / HD Graphics, and the integrated "Arc Graphics" on
/// Core Ultra (Meteor/Arrow Lake) chips — only *discrete* Arc cards carry an A-/B-series model token
/// (Arc A770, Arc B580). AMD APUs market as a bare "Radeon(TM) Graphics" or "Radeon Vega N Graphics",
/// while discrete AMD carries an explicit tier (RX / Pro / FirePro / Instinct). NVIDIA has no
/// integrated desktop/laptop part. Conservative: an unrecognized name → `false` (discrete), and AMD is
/// positive-matched on APU markers so a discrete card is never wrongly flagged (a missed newer APU
/// like a 780M usually reports no reliable VRAM on Windows anyway → no Split regardless).
#[cfg(any(windows, test))]
fn integrated_gpu_from_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    if n.contains("nvidia") || n.contains("geforce") || n.contains("rtx") || n.contains("gtx") {
        return false; // NVIDIA is always discrete.
    }
    if n.contains("intel") || n.contains("uhd") || n.contains("iris") {
        // "Arc Graphics" (no model token) is the Core-Ultra iGPU; "Arc A770"/"Arc B580" are discrete.
        if n.contains("arc") {
            return !has_arc_discrete_model(&n);
        }
        return true;
    }
    if n.contains("radeon") || n.contains("amd") {
        let discrete = n.contains(" rx ")
            || n.contains("radeon rx")
            || n.contains("radeon pro")
            || n.contains("firepro")
            || n.contains("instinct");
        return !discrete && (n.contains("graphics") || n.contains("vega"));
    }
    false
}

/// True if an already-lowercased Intel-Arc name carries a discrete A-/B-series model token — an `a`/`b`
/// followed by 2+ digits (`a60`, `a770`, `b580`) — as opposed to the integrated "Arc Graphics" that
/// carries none. Covers the desktop (A3xx–A7xx, B5xx) and workstation Arc Pro (A40/A50/A60) lines.
#[cfg(any(windows, test))]
fn has_arc_discrete_model(name_lower: &str) -> bool {
    name_lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|tok| {
            let b = tok.as_bytes();
            b.len() >= 3 && (b[0] == b'a' || b[0] == b'b') && b[1..].iter().all(u8::is_ascii_digit)
        })
}

/// VRAM available to an Apple-Silicon GPU: the working-set fraction of unified memory.
#[cfg(any(target_os = "macos", test))]
fn apple_vram_gb(total_gb: f64) -> f64 {
    total_gb * APPLE_VRAM_FRACTION
}

/// True when `/proc/version` marks a WSL kernel.
#[cfg(any(target_os = "linux", test))]
fn detect_wsl(proc_version: &str) -> bool {
    let v = proc_version.to_ascii_lowercase();
    v.contains("microsoft") || v.contains("wsl")
}

/// Is a `/sys/class/drm/cardN/device` symlink target an integrated GPU? Integrated GPUs sit on PCI
/// bus 00 (the CPU root complex); a discrete card is behind a PCIe port on a higher bus. The target
/// looks like `../../../0000:03:00.0` — the bus is the field between the two colons of the last path
/// segment. Pure; an unparseable target → `false` (treated as discrete).
#[cfg(any(target_os = "linux", test))]
fn pci_bus_is_integrated(link_target: &str) -> bool {
    link_target
        .rsplit('/')
        .next()
        .and_then(|addr| addr.split(':').nth(1)) // "0000:BB:DD.F" → "BB"
        .is_some_and(|bus| bus == "00")
}

// --- Intel DRM query: ABI transcription + pure blob parsing (#461) ------------------------------
//
// Everything below is `#[cfg(any(target_os = "linux", test))]` rather than Linux-only on purpose: it
// is the part of the ioctl path that CAN be checked without a discrete Arc, so it is compiled and
// unit-tested on every platform's `just check`, not just CI's Linux job.

/// `_IOWR(type, nr, argtype)` from `asm-generic/ioctl.h`: `dir(2) | size(14) | type(8) | nr(8)`.
/// Derived from `size_of` rather than hardcoded so a request word can never drift from the struct it
/// describes — a mismatch would make the kernel copy the wrong number of bytes. (The legacy
/// alpha/mips/parisc/powerpc/sparc encoding differs, but no discrete Intel GPU ships on those.)
#[cfg(any(target_os = "linux", test))]
const fn drm_iowr(nr: u32, arg_size: usize) -> u32 {
    const DIR_READ_WRITE: u32 = 3; // _IOC_READ | _IOC_WRITE
    const DRM_IOCTL_BASE: u32 = 0x64; // 'd'
    (DIR_READ_WRITE << 30) | ((arg_size as u32) << 16) | (DRM_IOCTL_BASE << 8) | nr
}

/// Driver-private ioctls start here (`drm.h`); each driver's command offset is added to it.
#[cfg(any(target_os = "linux", test))]
const DRM_COMMAND_BASE: u32 = 0x40;
/// i915's `DRM_I915_QUERY` command offset. xe's `DRM_XE_DEVICE_QUERY` offset is `0x00`, so its ioctl
/// nr is `DRM_COMMAND_BASE` itself — written that way below because adding a zero is a clippy error.
#[cfg(any(target_os = "linux", test))]
const DRM_I915_QUERY_CMD: u32 = 0x39;

#[cfg(any(target_os = "linux", test))]
const DRM_IOCTL_I915_QUERY: u32 = drm_iowr(
    DRM_COMMAND_BASE + DRM_I915_QUERY_CMD,
    std::mem::size_of::<DrmI915Query>(),
);
#[cfg(any(target_os = "linux", test))]
const DRM_IOCTL_XE_DEVICE_QUERY: u32 =
    drm_iowr(DRM_COMMAND_BASE, std::mem::size_of::<DrmXeDeviceQuery>());

/// `DRM_I915_QUERY_MEMORY_REGIONS`. Added in Linux 5.14; an older kernel answers `-EINVAL` as a
/// negative item length while the ioctl itself succeeds, which [`i915_blob_len`] rejects.
#[cfg(any(target_os = "linux", test))]
const DRM_I915_QUERY_MEMORY_REGIONS: u32 = 4;
/// `DRM_XE_DEVICE_QUERY_MEM_REGIONS`.
#[cfg(any(target_os = "linux", test))]
const DRM_XE_DEVICE_QUERY_MEM_REGIONS: u32 = 1;

/// `struct drm_i915_query` — the outer request. Its `size_of` is what the ioctl number encodes.
#[cfg(any(target_os = "linux", test))]
#[repr(C)]
#[derive(Default)]
#[allow(dead_code)] // Every field is read by the KERNEL across the ioctl, which rustc can't see.
struct DrmI915Query {
    num_items: u32,
    flags: u32,
    items_ptr: u64,
}

/// `struct drm_i915_query_item`. `length` is **signed**: the ioctl reports per-item failures as a
/// negative `-errno` here while itself returning 0, so it must never be read as unsigned.
#[cfg(any(target_os = "linux", test))]
#[repr(C)]
#[derive(Default)]
#[allow(dead_code)] // As above — `query_id` / `flags` / `data_ptr` are read across the syscall.
struct DrmI915QueryItem {
    query_id: u64,
    length: i32,
    flags: u32,
    data_ptr: u64,
}

/// `struct drm_xe_device_query` — the outer request, doubling as the length probe's reply (the
/// kernel writes the required byte count back into `size`).
#[cfg(any(target_os = "linux", test))]
#[repr(C)]
#[derive(Default)]
#[allow(dead_code)] // As above — `extensions` / `query` / `data` / `reserved` cross the syscall.
struct DrmXeDeviceQuery {
    extensions: u64,
    query: u32,
    size: u32,
    data: u64,
    reserved: [u64; 2],
}

/// `sizeof(struct drm_i915_query_memory_regions)` — the flexible-array header before the records.
#[cfg(any(target_os = "linux", test))]
const I915_REGIONS_HEADER: usize = 16;
/// `sizeof(struct drm_xe_query_mem_regions)` — ditto for xe (a `__u32` count plus a `__u32` pad).
#[cfg(any(target_os = "linux", test))]
const XE_REGIONS_HEADER: usize = 8;
/// Both drivers' per-region record happens to be 88 bytes with the class at +0 and the size at +8.
/// They are still independent ABIs — the shared constant is a convenience, never a guarantee, which
/// is why each driver keeps its own parser and its own fixture test.
#[cfg(any(target_os = "linux", test))]
const DRM_REGION_RECORD: usize = 88;
/// The device-local class in both enums (`I915_MEMORY_CLASS_DEVICE` / `DRM_XE_MEM_REGION_CLASS_VRAM`);
/// 0 is system memory in both. An integrated part reports no record of this class at all.
#[cfg(any(target_os = "linux", test))]
const DRM_MEMORY_CLASS_DEVICE: u16 = 1;
/// A generous ceiling on the region count, so a nonsense length can't drive a large allocation. Real
/// parts report 1 (integrated) to a handful (one per tile).
#[cfg(any(target_os = "linux", test))]
const MAX_DRM_REGIONS: usize = 64;

/// Validate the byte count i915 reported in `item.length`. Rejects the negative `-errno` an older
/// kernel returns, and any length that isn't a whole header-plus-records blob of plausible size.
#[cfg(any(target_os = "linux", test))]
fn i915_blob_len(reported: i32) -> Option<usize> {
    let len = usize::try_from(reported).ok()?; // negative ⇒ -errno ⇒ not a length
    drm_blob_len_ok(len, I915_REGIONS_HEADER).then_some(len)
}

/// Validate the byte count xe wrote back into `query.size`.
#[cfg(any(target_os = "linux", test))]
fn xe_blob_len(reported: u32) -> Option<usize> {
    let len = reported as usize;
    drm_blob_len_ok(len, XE_REGIONS_HEADER).then_some(len)
}

#[cfg(any(target_os = "linux", test))]
fn drm_blob_len_ok(len: usize, header: usize) -> bool {
    len >= header
        && (len - header).is_multiple_of(DRM_REGION_RECORD)
        && len <= header + DRM_REGION_RECORD * MAX_DRM_REGIONS
}

/// Total `probed_size` across i915's device-local regions, or `None` when there are none (an
/// integrated GPU, which is the signal that there's no discrete pool to report).
#[cfg(any(target_os = "linux", test))]
fn parse_i915_vram_bytes(blob: &[u8]) -> Option<u64> {
    sum_device_local_regions(blob, I915_REGIONS_HEADER)
}

/// Total `total_size` across xe's VRAM regions, or `None` when there are none.
#[cfg(any(target_os = "linux", test))]
fn parse_xe_vram_bytes(blob: &[u8]) -> Option<u64> {
    sum_device_local_regions(blob, XE_REGIONS_HEADER)
}

/// Sum the device-local regions of a memory-regions reply blob. The record count is read from the
/// header but **clamped to what the blob can actually hold**, so a kernel (or a fixture) claiming
/// more records than it sent can't read past the end. Summed rather than first-wins: a multi-tile
/// part exposes one device-local region per tile. Values are native-endian, as the kernel wrote them.
#[cfg(any(target_os = "linux", test))]
fn sum_device_local_regions(blob: &[u8], header: usize) -> Option<u64> {
    if blob.len() < header {
        return None;
    }
    let claimed = u32::from_ne_bytes(blob[0..4].try_into().ok()?) as usize;
    let capacity = (blob.len() - header) / DRM_REGION_RECORD;
    let mut total: u64 = 0;
    let mut found = false;
    for i in 0..claimed.min(capacity) {
        let rec = &blob[header + i * DRM_REGION_RECORD..][..DRM_REGION_RECORD];
        let class = u16::from_ne_bytes(rec[0..2].try_into().ok()?);
        if class != DRM_MEMORY_CLASS_DEVICE {
            continue;
        }
        found = true;
        total = total.saturating_add(u64::from_ne_bytes(rec[8..16].try_into().ok()?));
    }
    (found && total > 0).then_some(total)
}

/// `0x8086` is Intel's PCI vendor id, as sysfs renders it (`0x8086\n`).
#[cfg(any(target_os = "linux", test))]
fn is_intel_pci_vendor(sysfs_value: &str) -> bool {
    sysfs_value.trim().eq_ignore_ascii_case("0x8086")
}

/// Which Intel driver a `/sys/class/drm/<node>/device/driver` symlink target names. Anything else —
/// `i915_bpo`, a vendor out-of-tree module, another vendor entirely — is `None`, so we never issue a
/// driver-specific ioctl at a driver whose ABI we don't know.
#[cfg(any(target_os = "linux", test))]
fn intel_drm_driver_from_link(link_target: &str) -> Option<IntelDrmDriver> {
    match link_target.trim_end_matches('/').rsplit('/').next()? {
        "i915" => Some(IntelDrmDriver::I915),
        "xe" => Some(IntelDrmDriver::Xe),
        _ => None,
    }
}

// --- GPU memory bandwidth: name (+ VRAM) → GB/s, for the tok/s speed estimate ------------------
//
// Peak theoretical VRAM bandwidth (GB/s) per discrete GPU, from manufacturer / TechPowerUp specs
// (verified 2026-07). Decode of a memory-bound LLM runs at ~bandwidth / active-weight-bytes-per-token,
// so a real per-card number turns fit.rs's flat 400-GB/s placeholder into a card-specific estimate (an
// RTX 4090 ≈ 1008 vs an Arc A380 ≈ 186). Maintenance notes:
//   * Keys are normalised (see `normalize_gpu_name`): lowercase, every non-alphanumeric run collapsed
//     to one space. Match is substring containment, LONGEST key first, so "rtx 4080 super" beats the
//     "rtx 4080" it contains, and "rx 7900 xtx" beats "rx 7900 xt". Add specific-before-generic; the
//     longest-match rule handles the overlap.
//   * A few marketing names ship two bandwidths that differ by memory capacity (RTX 3060, RTX 3080,
//     Arc A770). Those use [`Bandwidth::ByVram`] and are resolved with the probed VRAM; when VRAM is
//     unknown we return `None` (flat fallback) rather than guess the variant.
//   * Discrete only. Integrated GPUs are flagged `unified_memory` and never reach the GPU-speed path;
//     Apple Silicon reports a generic name and stays on the default too (its bandwidth spans M1→Ultra).
//     Datacenter parts (A100/H100) are deliberately omitted — no PM desktop runs one, and their
//     SXM-vs-PCIe name variants would only add substring hazards.
#[derive(Debug, Clone, Copy)]
enum Bandwidth {
    /// One bandwidth for every card that reports this name.
    Fixed(f64),
    /// `high` GB/s at/above `threshold_gb` of VRAM, else `low` — for a name whose bus width (hence
    /// bandwidth) differs by memory capacity, disambiguated by the probed VRAM.
    ByVram {
        threshold_gb: f64,
        high: f64,
        low: f64,
    },
}

const GPU_BANDWIDTH_TABLE: &[(&str, Bandwidth)] = &[
    // ---- NVIDIA GeForce RTX 50-series ----
    ("rtx 5090", Bandwidth::Fixed(1792.0)),
    ("rtx 5080", Bandwidth::Fixed(960.0)),
    ("rtx 5070 ti", Bandwidth::Fixed(896.0)),
    ("rtx 5070", Bandwidth::Fixed(672.0)),
    ("rtx 5060 ti", Bandwidth::Fixed(448.0)), // 8 GB and 16 GB both 448
    ("rtx 5060", Bandwidth::Fixed(448.0)),
    // ---- NVIDIA GeForce RTX 40-series ----
    ("rtx 4090", Bandwidth::Fixed(1008.0)),
    ("rtx 4080 super", Bandwidth::Fixed(736.0)),
    ("rtx 4080", Bandwidth::Fixed(717.0)),
    ("rtx 4070 ti super", Bandwidth::Fixed(672.0)),
    ("rtx 4070 ti", Bandwidth::Fixed(504.0)),
    ("rtx 4070 super", Bandwidth::Fixed(504.0)),
    ("rtx 4070", Bandwidth::Fixed(504.0)),
    ("rtx 4060 ti", Bandwidth::Fixed(288.0)), // 8 GB and 16 GB both 288
    ("rtx 4060", Bandwidth::Fixed(272.0)),
    // ---- NVIDIA GeForce RTX 30-series ----
    ("rtx 3090 ti", Bandwidth::Fixed(1008.0)),
    ("rtx 3090", Bandwidth::Fixed(936.0)),
    ("rtx 3080 ti", Bandwidth::Fixed(912.0)),
    (
        "rtx 3080",
        Bandwidth::ByVram {
            threshold_gb: 11.0,
            high: 912.0,
            low: 760.0,
        },
    ), // 12 GB = 912, 10 GB = 760
    ("rtx 3070 ti", Bandwidth::Fixed(608.0)),
    ("rtx 3070", Bandwidth::Fixed(448.0)),
    ("rtx 3060 ti", Bandwidth::Fixed(448.0)),
    (
        "rtx 3060",
        Bandwidth::ByVram {
            threshold_gb: 10.0,
            high: 360.0,
            low: 240.0,
        },
    ), // 12 GB = 360, 8 GB = 240
    ("rtx 3050", Bandwidth::Fixed(224.0)),
    // ---- NVIDIA GeForce RTX 20-series ----
    ("rtx 2080 ti", Bandwidth::Fixed(616.0)),
    ("rtx 2080 super", Bandwidth::Fixed(496.0)),
    ("rtx 2070 super", Bandwidth::Fixed(448.0)),
    ("rtx 2060", Bandwidth::Fixed(336.0)),
    // ---- NVIDIA single-GPU workstation ----
    ("rtx 6000 ada", Bandwidth::Fixed(960.0)),
    ("rtx a6000", Bandwidth::Fixed(768.0)),
    ("rtx a5000", Bandwidth::Fixed(768.0)),
    ("rtx a4000", Bandwidth::Fixed(448.0)),
    ("l40s", Bandwidth::Fixed(864.0)),
    // ---- AMD Radeon RX 9000 / 7000 ----
    ("radeon rx 9070 xt", Bandwidth::Fixed(640.0)),
    ("radeon rx 9070", Bandwidth::Fixed(640.0)),
    ("radeon rx 7900 xtx", Bandwidth::Fixed(960.0)),
    ("radeon rx 7900 xt", Bandwidth::Fixed(800.0)),
    ("radeon rx 7900 gre", Bandwidth::Fixed(576.0)),
    ("radeon rx 7800 xt", Bandwidth::Fixed(624.0)),
    ("radeon rx 7700 xt", Bandwidth::Fixed(432.0)),
    ("radeon rx 7600", Bandwidth::Fixed(288.0)),
    // ---- AMD Radeon RX 6000 ----
    ("radeon rx 6950 xt", Bandwidth::Fixed(576.0)),
    ("radeon rx 6900 xt", Bandwidth::Fixed(512.0)),
    ("radeon rx 6800 xt", Bandwidth::Fixed(512.0)),
    ("radeon rx 6800", Bandwidth::Fixed(512.0)),
    ("radeon rx 6750 xt", Bandwidth::Fixed(432.0)),
    ("radeon rx 6700 xt", Bandwidth::Fixed(384.0)),
    ("radeon rx 6600 xt", Bandwidth::Fixed(256.0)),
    ("radeon rx 6600", Bandwidth::Fixed(224.0)),
    ("radeon pro w7900", Bandwidth::Fixed(864.0)),
    ("radeon pro w7800", Bandwidth::Fixed(576.0)),
    // ---- Intel Arc ----
    ("arc b580", Bandwidth::Fixed(456.0)),
    ("arc b570", Bandwidth::Fixed(380.0)),
    (
        "arc a770",
        Bandwidth::ByVram {
            threshold_gb: 12.0,
            high: 560.0,
            low: 512.0,
        },
    ), // 16 GB = 560, 8 GB = 512
    ("arc a750", Bandwidth::Fixed(512.0)),
    ("arc a580", Bandwidth::Fixed(512.0)),
    ("arc a380", Bandwidth::Fixed(186.0)),
    ("arc a310", Bandwidth::Fixed(124.0)),
];

/// Normalise a GPU name for table matching: lowercase, and collapse every run of non-alphanumeric
/// characters (spaces, `(R)`, `(TM)`, hyphens, punctuation) to a single space. So `"AMD Radeon(TM) RX
/// 7900 XTX"` and `"Intel(R) Arc(TM) A770 Graphics"` reduce to `"amd radeon rx 7900 xtx"` / `"intel arc
/// a770 graphics"`, which the model-token keys (`"radeon rx 7900 xtx"`, `"arc a770"`) match by
/// containment.
fn normalize_gpu_name(name: &str) -> String {
    // Drop the trademark markers FIRST: their letters (`(TM)` → `tm`, `(R)` → `r`) would otherwise
    // survive as tokens wedged between the model words and break substring matching.
    let cleaned = name
        .to_ascii_lowercase()
        .replace("(tm)", " ")
        .replace("(r)", " ")
        .replace(['™', '®'], " ");
    let mut out = String::with_capacity(cleaned.len());
    let mut prev_space = true; // leading true → no leading space
    for c in cleaned.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            prev_space = false;
        } else if !prev_space {
            out.push(' ');
            prev_space = true;
        }
    }
    out.trim_end().to_string()
}

/// The best (longest, most specific) bandwidth entry whose key is a substring of the normalised name —
/// so `"rtx 4080 super"` wins over the `"rtx 4080"` it contains. Returns the unresolved [`Bandwidth`]
/// (a `ByVram` entry still needs the VRAM to pick a number). `table` is a parameter so the match logic
/// is unit-testable independent of the real data.
fn match_bandwidth(name: &str, table: &[(&str, Bandwidth)]) -> Option<Bandwidth> {
    let n = normalize_gpu_name(name);
    table
        .iter()
        .filter(|(key, _)| n.contains(key))
        .max_by_key(|(key, _)| key.len())
        .map(|&(_, bw)| bw)
}

/// The GPU's peak memory bandwidth (GB/s) from its reported name and VRAM, or `None` for an
/// unrecognised card — or a capacity-ambiguous one whose VRAM we couldn't read (the caller then falls
/// back to fit-scoring's flat default). Pure; safe on every platform.
pub fn gpu_bandwidth_gbps(name: &str, vram_gb: Option<f64>) -> Option<f64> {
    match match_bandwidth(name, GPU_BANDWIDTH_TABLE)? {
        Bandwidth::Fixed(gbps) => Some(gbps),
        Bandwidth::ByVram {
            threshold_gb,
            high,
            low,
        } => vram_gb.map(|v| if v >= threshold_gb { high } else { low }),
    }
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_object_and_array_video_controllers() {
        let single =
            r#"{"Name":"NVIDIA GeForce RTX 4070","AdapterRAM":2147483648,"DriverVersion":"1.2"}"#;
        let got = parse_video_controller_json(single);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "NVIDIA GeForce RTX 4070");
        assert_eq!(got[0].adapter_ram, Some(2_147_483_648));

        let array = r#"[{"Name":"Intel UHD","AdapterRAM":1073741824},{"Name":"NVIDIA RTX","AdapterRAM":4294967295}]"#;
        let got = parse_video_controller_json(array);
        assert_eq!(got.len(), 2);
        assert_eq!(got[1].adapter_ram, Some(4_294_967_295));
    }

    #[test]
    fn video_controller_handles_stringified_and_missing_adapter_ram() {
        let mixed =
            r#"[{"Name":"A","AdapterRAM":"2147483648"},{"Name":"B"},{"Name":"C","AdapterRAM":0}]"#;
        let got = parse_video_controller_json(mixed);
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].adapter_ram, Some(2_147_483_648));
        assert_eq!(got[1].adapter_ram, None);
        assert_eq!(got[2].adapter_ram, None); // 0 filtered out
    }

    #[test]
    fn video_controller_rejects_garbage() {
        assert!(parse_video_controller_json("not json").is_empty());
        assert!(parse_video_controller_json("42").is_empty());
        assert!(parse_video_controller_json("").is_empty());
    }

    #[test]
    fn pick_gpu_prefers_the_discrete_card_over_basic_display() {
        let lines = vec![
            GpuLine {
                name: "Microsoft Basic Display Adapter".into(),
                adapter_ram: None,
            },
            GpuLine {
                name: "NVIDIA RTX".into(),
                adapter_ram: Some(2_000_000_000),
            },
        ];
        assert_eq!(pick_gpu(&lines).unwrap().name, "NVIDIA RTX");
    }

    #[test]
    fn pick_gpu_falls_back_to_basic_when_thats_all_there_is() {
        let lines = vec![GpuLine {
            name: "Microsoft Basic Display Adapter".into(),
            adapter_ram: None,
        }];
        assert_eq!(
            pick_gpu(&lines).unwrap().name,
            "Microsoft Basic Display Adapter"
        );
        assert!(pick_gpu(&[]).is_none());
    }

    #[test]
    fn adapter_ram_trusts_small_values_and_distrusts_the_uint32_ceiling() {
        assert!(adapter_ram_reliable(2_147_483_648)); // 2 GiB — trustworthy
        assert!(!adapter_ram_reliable(4_294_967_295)); // saturated uint32 — a lie
        assert!(!adapter_ram_reliable(0));
    }

    #[test]
    fn nvidia_smi_takes_the_largest_gpu() {
        assert_eq!(parse_nvidia_smi_csv("8192\n24576\n"), Some(24576.0));
        assert_eq!(parse_nvidia_smi_csv("12288"), Some(12288.0));
        assert_eq!(parse_nvidia_smi_csv(""), None);
        assert_eq!(parse_nvidia_smi_csv("no gpu found"), None);
    }

    #[test]
    fn vendor_is_guessed_from_the_name() {
        assert_eq!(
            vendor_from_name("NVIDIA GeForce RTX 4070").as_deref(),
            Some("NVIDIA")
        );
        assert_eq!(
            vendor_from_name("AMD Radeon RX 7900").as_deref(),
            Some("AMD")
        );
        assert_eq!(vendor_from_name("Intel Arc A770").as_deref(), Some("Intel"));
        assert_eq!(vendor_from_name("Apple M3 Pro").as_deref(), Some("Apple"));
        assert_eq!(vendor_from_name("Some Unknown Adapter"), None);
    }

    #[test]
    fn apple_vram_is_a_fraction_of_unified_memory() {
        assert!((apple_vram_gb(16.0) - 12.0).abs() < 1e-9);
    }

    #[test]
    fn wsl_is_detected_from_proc_version() {
        assert!(detect_wsl("Linux version 5.15.0-microsoft-standard-WSL2"));
        assert!(detect_wsl("... Microsoft ..."));
        assert!(!detect_wsl("Linux version 6.1.0-generic (gcc ...)"));
    }

    #[test]
    fn integrated_gpu_is_classified_from_the_name() {
        // Intel iGPUs are integrated, incl. the Core-Ultra "Arc Graphics"; discrete Arc has a model.
        assert!(integrated_gpu_from_name("Intel(R) UHD Graphics 770"));
        assert!(integrated_gpu_from_name("Intel(R) Iris(R) Xe Graphics"));
        assert!(integrated_gpu_from_name("Intel(R) Arc(TM) Graphics")); // Core-Ultra iGPU
        assert!(!integrated_gpu_from_name("Intel(R) Arc(TM) A770 Graphics")); // discrete
        assert!(!integrated_gpu_from_name("Intel Arc B580")); // discrete
        assert!(!integrated_gpu_from_name("Intel Arc Pro A60")); // discrete workstation (2-digit model)
                                                                 // AMD APUs read as bare "... Graphics" / "Vega N"; discrete AMD carries a tier.
        assert!(integrated_gpu_from_name("AMD Radeon(TM) Graphics"));
        assert!(integrated_gpu_from_name("AMD Radeon(TM) Vega 8 Graphics"));
        assert!(!integrated_gpu_from_name("AMD Radeon RX 7900 XT"));
        assert!(!integrated_gpu_from_name("AMD Radeon Pro W6800"));
        // NVIDIA has no integrated part; unknown names are conservatively discrete.
        assert!(!integrated_gpu_from_name(
            "NVIDIA GeForce RTX 5070 Laptop GPU"
        ));
        assert!(!integrated_gpu_from_name("Some Unknown Adapter"));
    }

    #[test]
    fn dxgi_picks_the_largest_discrete_skipping_software_and_igpu_carveouts() {
        fn adapter(name: &str, vendor_id: u32, bytes: u64, is_software: bool) -> DxgiAdapter {
            DxgiAdapter {
                name: name.into(),
                vendor_id,
                dedicated_bytes: bytes,
                is_software,
            }
        }
        let adapters = vec![
            adapter("Intel Arc B580", 0x8086, 12 * 1_073_741_824, false), // 12 GiB discrete
            adapter("Intel UHD Graphics", 0x8086, 128 * 1_048_576, false), // 128 MiB iGPU carve-out
            adapter("Microsoft Basic Render Driver", 0x1414, 0, true),    // WARP — skipped
        ];
        let picked = pick_dxgi_discrete(&adapters).unwrap();
        assert_eq!(picked.name, "Intel Arc B580");
        // The picked adapter's vendor id resolves (this also exercises the DXGI-only vendor fallback).
        assert_eq!(
            vendor_from_pci_id(picked.vendor_id).as_deref(),
            Some("Intel")
        );

        // A huge software adapter is still skipped; a sub-floor iGPU alone yields nothing. A real
        // iGPU reports a ~128 MiB BIOS carve-out here (verified live on Iris Xe), well under the floor.
        assert!(pick_dxgi_discrete(&[adapter("WARP", 0x1414, 64 * 1_073_741_824, true)]).is_none());
        assert!(
            pick_dxgi_discrete(&[adapter("Intel Iris Xe", 0x8086, 512 * 1_048_576, false)])
                .is_none()
        );
        assert!(pick_dxgi_discrete(&[]).is_none());
    }

    #[test]
    fn utf16_description_is_decoded_and_nul_trimmed() {
        // "Arc" followed by a NUL and trailing padding — decode stops at the NUL, trims the rest.
        let mut buf = [0u16; 8];
        for (slot, ch) in buf.iter_mut().zip("Arc".encode_utf16()) {
            *slot = ch;
        }
        assert_eq!(utf16_trim_to_string(&buf), "Arc");
        assert_eq!(utf16_trim_to_string(&[0u16; 4]), ""); // all-NUL → empty
        assert_eq!(utf16_trim_to_string(&[]), "");
    }

    #[test]
    fn vendor_from_pci_id_maps_the_three_gpu_vendors() {
        assert_eq!(vendor_from_pci_id(0x10DE).as_deref(), Some("NVIDIA"));
        assert_eq!(vendor_from_pci_id(0x1002).as_deref(), Some("AMD"));
        assert_eq!(vendor_from_pci_id(0x8086).as_deref(), Some("Intel"));
        assert_eq!(vendor_from_pci_id(0x1414), None); // Microsoft WARP
        assert_eq!(vendor_from_pci_id(0), None);
    }

    #[test]
    fn normalize_gpu_name_strips_vendor_noise_and_collapses_space() {
        assert_eq!(
            normalize_gpu_name("AMD Radeon(TM) RX 7900 XTX"),
            "amd radeon rx 7900 xtx"
        );
        assert_eq!(
            normalize_gpu_name("Intel(R) Arc(TM) A770 Graphics"),
            "intel arc a770 graphics"
        );
        assert_eq!(
            normalize_gpu_name("  NVIDIA   GeForce  RTX 4090 "),
            "nvidia geforce rtx 4090"
        );
    }

    #[test]
    fn bandwidth_prefers_the_more_specific_model_name() {
        // Longest-match on real data: "4080 super" (736) must beat the "rtx 4080" (717) it contains,
        // and "7900 xtx" (960) must beat the "7900 xt" (800) it contains — the two classic collisions.
        let v = Some(16.0);
        assert_eq!(
            gpu_bandwidth_gbps("NVIDIA GeForce RTX 4080 SUPER", v),
            Some(736.0)
        );
        assert_eq!(
            gpu_bandwidth_gbps("NVIDIA GeForce RTX 4080", v),
            Some(717.0)
        );
        assert_eq!(
            gpu_bandwidth_gbps("AMD Radeon RX 7900 XTX", Some(24.0)),
            Some(960.0)
        );
        assert_eq!(
            gpu_bandwidth_gbps("AMD Radeon RX 7900 XT", Some(20.0)),
            Some(800.0)
        );
    }

    #[test]
    fn bandwidth_disambiguates_shared_names_by_vram() {
        // Same reported name, two bus widths → the probed VRAM picks the variant.
        assert_eq!(
            gpu_bandwidth_gbps("NVIDIA GeForce RTX 3060", Some(12.0)),
            Some(360.0)
        );
        assert_eq!(
            gpu_bandwidth_gbps("NVIDIA GeForce RTX 3060", Some(8.0)),
            Some(240.0)
        );
        assert_eq!(
            gpu_bandwidth_gbps("NVIDIA GeForce RTX 3080", Some(12.0)),
            Some(912.0)
        );
        assert_eq!(
            gpu_bandwidth_gbps("NVIDIA GeForce RTX 3080", Some(10.0)),
            Some(760.0)
        );
        assert_eq!(
            gpu_bandwidth_gbps("Intel(R) Arc(TM) A770 Graphics", Some(16.0)),
            Some(560.0)
        );
        assert_eq!(
            gpu_bandwidth_gbps("Intel(R) Arc(TM) A770 Graphics", Some(8.0)),
            Some(512.0)
        );
        // A 3080 Ti still resolves to its own fixed value (the more specific key wins over "rtx 3080").
        assert_eq!(
            gpu_bandwidth_gbps("NVIDIA GeForce RTX 3080 Ti", Some(12.0)),
            Some(912.0)
        );
        // Ambiguous name with unreadable VRAM → don't guess the variant → flat fallback.
        assert_eq!(gpu_bandwidth_gbps("NVIDIA GeForce RTX 3060", None), None);
    }

    #[test]
    fn unknown_or_generic_gpu_has_no_calibrated_bandwidth() {
        // A generic name (Linux nvidia-smi) or an unlisted card → None → fit-scoring's flat default.
        assert_eq!(gpu_bandwidth_gbps("NVIDIA GPU", Some(24.0)), None);
        assert_eq!(
            gpu_bandwidth_gbps("Some Unlisted Card 9999", Some(8.0)),
            None
        );
        assert_eq!(
            gpu_bandwidth_gbps("Intel(R) Arc(TM) Graphics", Some(2.0)),
            None
        ); // Core-Ultra iGPU
    }

    // --- Intel DRM query (#461) ---------------------------------------------------------------
    //
    // The ioctl path itself can't be exercised without a discrete-Arc-on-Linux machine, so these
    // pin everything that CAN be checked: the transcribed struct layout, the request words derived
    // from it, the length validation that stands between a kernel reply and an allocation, and the
    // blob parsing. Each request-word constant is asserted against the value independently computed
    // from the kernel headers — if a field type or order below ever drifts, `size_of` changes and
    // these fail rather than silently making the kernel copy the wrong number of bytes.

    /// One memory-regions reply blob, laid out as the kernel writes it: a `header`-byte head whose
    /// first `u32` is the record count, then one 88-byte record per `(class, size_bytes)` pair.
    fn drm_regions_blob(header: usize, regions: &[(u16, u64)]) -> Vec<u8> {
        let mut blob = vec![0u8; header + regions.len() * DRM_REGION_RECORD];
        blob[0..4].copy_from_slice(&(regions.len() as u32).to_ne_bytes());
        for (i, &(class, bytes)) in regions.iter().enumerate() {
            let at = header + i * DRM_REGION_RECORD;
            blob[at..at + 2].copy_from_slice(&class.to_ne_bytes());
            blob[at + 8..at + 16].copy_from_slice(&bytes.to_ne_bytes());
        }
        blob
    }

    #[test]
    fn drm_request_words_match_the_kernel_headers() {
        // _IOWR('d', 0x40 + 0x39, struct drm_i915_query)      → dir 3, size 16, type 0x64, nr 0x79
        assert_eq!(DRM_IOCTL_I915_QUERY, 0xC010_6479);
        // _IOWR('d', 0x40 + 0x00, struct drm_xe_device_query) → dir 3, size 40, type 0x64, nr 0x40
        assert_eq!(DRM_IOCTL_XE_DEVICE_QUERY, 0xC028_6440);
        // The encoding itself, independent of our structs: dir<<30 | size<<16 | type<<8 | nr.
        assert_eq!(
            drm_iowr(0x79, 16),
            (3 << 30) | (16 << 16) | (0x64 << 8) | 0x79
        );
        // The query ids those requests carry, from the same headers. Asserted here rather than left
        // to the Linux-only call sites so that every platform's test run pins them.
        assert_eq!(DRM_I915_QUERY_MEMORY_REGIONS, 4);
        assert_eq!(DRM_XE_DEVICE_QUERY_MEM_REGIONS, 1);
    }

    #[test]
    fn drm_structs_match_the_uapi_layout() {
        use std::mem::{align_of, offset_of, size_of};

        assert_eq!(size_of::<DrmI915Query>(), 16);
        assert_eq!(offset_of!(DrmI915Query, num_items), 0);
        assert_eq!(offset_of!(DrmI915Query, flags), 4);
        assert_eq!(offset_of!(DrmI915Query, items_ptr), 8);

        assert_eq!(size_of::<DrmI915QueryItem>(), 24);
        assert_eq!(offset_of!(DrmI915QueryItem, query_id), 0);
        assert_eq!(offset_of!(DrmI915QueryItem, length), 8);
        assert_eq!(offset_of!(DrmI915QueryItem, flags), 12);
        assert_eq!(offset_of!(DrmI915QueryItem, data_ptr), 16);

        assert_eq!(size_of::<DrmXeDeviceQuery>(), 40);
        assert_eq!(offset_of!(DrmXeDeviceQuery, extensions), 0);
        assert_eq!(offset_of!(DrmXeDeviceQuery, query), 8);
        assert_eq!(offset_of!(DrmXeDeviceQuery, size), 12);
        assert_eq!(offset_of!(DrmXeDeviceQuery, data), 16);
        assert_eq!(offset_of!(DrmXeDeviceQuery, reserved), 24);

        // The reply buffers are allocated as `u64` words, which is only sound if every blob length
        // is a whole number of them — true of 16 + 88N and 8 + 88N alike.
        assert_eq!(align_of::<u64>(), 8);
        assert_eq!(I915_REGIONS_HEADER % 8, 0);
        assert_eq!(XE_REGIONS_HEADER % 8, 0);
        assert_eq!(DRM_REGION_RECORD % 8, 0);
    }

    #[test]
    fn i915_blob_len_rejects_the_negative_errno_and_nonsense() {
        // A pre-5.14 kernel has no memory-regions query and reports -EINVAL *as the length*, while
        // the ioctl itself returns 0. Read unsigned, that becomes 4294967274 — a 4 GiB allocation.
        assert_eq!(i915_blob_len(-22), None);
        assert_eq!(i915_blob_len(-1), None);
        // Real shapes: header + N records.
        assert_eq!(i915_blob_len(16), Some(16)); // zero regions is a well-formed (if empty) blob
        assert_eq!(i915_blob_len(16 + 88), Some(104)); // integrated: system memory only
        assert_eq!(i915_blob_len(16 + 88 * 2), Some(192)); // discrete: system + device-local
                                                           // Not a whole number of records, or implausibly large.
        assert_eq!(i915_blob_len(100), None);
        assert_eq!(i915_blob_len(8), None);
        assert_eq!(i915_blob_len(16 + 88 * 65), None);
    }

    #[test]
    fn xe_blob_len_validates_the_kernels_reported_size() {
        assert_eq!(xe_blob_len(8), Some(8));
        assert_eq!(xe_blob_len(96), Some(96)); // integrated: SYSMEM only
        assert_eq!(xe_blob_len(184), Some(184)); // single-tile discrete: SYSMEM + VRAM
        assert_eq!(xe_blob_len(272), Some(272)); // two-tile
        assert_eq!(xe_blob_len(0), None);
        assert_eq!(xe_blob_len(100), None);
        assert_eq!(xe_blob_len(8 + 88 * 65), None);
    }

    #[test]
    fn drm_parsers_sum_device_local_regions_only() {
        // A discrete card reports system memory (class 0) alongside its VRAM (class 1); only the
        // latter is a distinct pool. 16 GiB, as an Arc A770 16 GB or a B580 would report.
        const VRAM: u64 = 16 * 1_073_741_824;
        let i915 = drm_regions_blob(I915_REGIONS_HEADER, &[(0, 34_000_000_000), (1, VRAM)]);
        assert_eq!(parse_i915_vram_bytes(&i915), Some(VRAM));
        let xe = drm_regions_blob(XE_REGIONS_HEADER, &[(0, 34_000_000_000), (1, VRAM)]);
        assert_eq!(parse_xe_vram_bytes(&xe), Some(VRAM));

        // Multi-tile: one device-local region per tile, summed rather than first-wins.
        let two_tile = drm_regions_blob(XE_REGIONS_HEADER, &[(0, 8_000_000_000), (1, 4), (1, 6)]);
        assert_eq!(parse_xe_vram_bytes(&two_tile), Some(10));
    }

    #[test]
    fn an_integrated_intel_gpu_reports_no_device_local_region() {
        // This is the discrete gate: an iGPU answers with system memory only, so there is nothing to
        // report and the scan falls back to sizing on system RAM — never a fabricated "VRAM".
        let igpu = drm_regions_blob(I915_REGIONS_HEADER, &[(0, 34_000_000_000)]);
        assert_eq!(parse_i915_vram_bytes(&igpu), None);
        assert_eq!(
            parse_xe_vram_bytes(&drm_regions_blob(XE_REGIONS_HEADER, &[(0, 34_000_000_000)])),
            None
        );
        // A device-local region that reports zero bytes is not a usable reading either.
        assert_eq!(
            parse_i915_vram_bytes(&drm_regions_blob(I915_REGIONS_HEADER, &[(1, 0)])),
            None
        );
    }

    #[test]
    fn drm_parsers_refuse_to_read_past_a_lying_region_count() {
        // The count is the kernel's claim; the blob length is the fact. Clamp to the fact.
        let mut blob = drm_regions_blob(I915_REGIONS_HEADER, &[(1, 512)]);
        blob[0..4].copy_from_slice(&99u32.to_ne_bytes());
        assert_eq!(parse_i915_vram_bytes(&blob), Some(512));

        // Truncated or absent headers yield nothing rather than panicking.
        assert_eq!(parse_i915_vram_bytes(&[]), None);
        assert_eq!(parse_i915_vram_bytes(&[0u8; 8]), None);
        assert_eq!(parse_xe_vram_bytes(&[0u8; 4]), None);
        // A header promising a record the blob doesn't contain.
        let mut short = vec![0u8; XE_REGIONS_HEADER];
        short[0..4].copy_from_slice(&1u32.to_ne_bytes());
        assert_eq!(parse_xe_vram_bytes(&short), None);
    }

    #[test]
    fn intel_drm_nodes_are_matched_by_vendor_and_driver() {
        assert!(is_intel_pci_vendor("0x8086\n"));
        assert!(is_intel_pci_vendor(" 0X8086 "));
        assert!(!is_intel_pci_vendor("0x10de")); // NVIDIA
        assert!(!is_intel_pci_vendor("0x1002")); // AMD
        assert!(!is_intel_pci_vendor(""));

        assert_eq!(
            intel_drm_driver_from_link("../../../../bus/pci/drivers/i915"),
            Some(IntelDrmDriver::I915)
        );
        assert_eq!(
            intel_drm_driver_from_link("../../../../bus/pci/drivers/xe"),
            Some(IntelDrmDriver::Xe)
        );
        // An unknown driver is never issued a driver-specific ioctl.
        assert_eq!(
            intel_drm_driver_from_link("/sys/bus/pci/drivers/amdgpu"),
            None
        );
        assert_eq!(intel_drm_driver_from_link("i915_bpo"), None);
        assert_eq!(intel_drm_driver_from_link(""), None);

        assert_eq!(IntelDrmDriver::I915.source_label(), "drm_i915");
        assert_eq!(IntelDrmDriver::Xe.source_label(), "drm_xe");
    }

    #[test]
    fn pci_bus_00_is_integrated_higher_buses_are_discrete() {
        assert!(pci_bus_is_integrated("../../../0000:00:02.0")); // iGPU on the root complex
        assert!(pci_bus_is_integrated("0000:00:08.1")); // bare last-segment form
        assert!(!pci_bus_is_integrated("../../../0000:03:00.0")); // discrete behind a PCIe port
        assert!(!pci_bus_is_integrated("../../../0000:01:00.0"));
        assert!(!pci_bus_is_integrated("")); // unparseable → discrete
        assert!(!pci_bus_is_integrated("garbage"));
    }
}
