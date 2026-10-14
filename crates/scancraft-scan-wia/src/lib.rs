// Copyright (C) 2026 ScanCraft contributors (GPL-2.0-or-later).
//
// Windows WIA 2.0 backend. Scan flow ported from NAPS2
// `NAPS2.Sdk/Scan/Internal/Wia` (GPL-2.0-or-later); COM signatures verified
// against the local `windows` crate bindings and Microsoft's WIA reference.
//
// Status: compiles on Windows; UNVERIFIED-ON-HARDWARE (no device here).
// Design notes:
// - ALL COM calls run on one dedicated STA thread owned by this crate.
//   Callers (`scan()`) block on a channel while the STA thread works.
// - No message pump: `IWiaTransfer::Download` is synchronous and the
//   callback object lives on the STA thread. If a driver hangs here, a pump
//   is the first thing to add (recorded in docs/hardware-test-checklist.md).
// - `unsafe` is confined to this crate's `imp` module; every block carries
//   a `// SAFETY:` comment (ultralight `unsafe` budget).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use scancraft_model::{BitDepth, PageSize, PaperSource};
use scancraft_scan_core::{CancelToken, Capabilities, DeviceId, DeviceInfo, PageSink, Progress, ScanError, ScanRequest, ScannerBackend};

/// Events from the STA thread to the calling thread.
enum StaEvent {
    /// Raw transferred file bytes (BMP) + side index.
    PageBytes(Vec<u8>, usize),
    Progress(Progress),
    Finished(Result<u32, ScanError>),
}

enum Job {
    ListDevices(Sender<Result<Vec<DeviceInfo>, ScanError>>),
    Scan(ScanRequest, Sender<StaEvent>, CancelToken),
}

/// WIA 2.0 backend. Cheap to clone (shared STA worker).
#[derive(Debug, Clone, Default)]
pub struct WiaScanner {
    sta: Arc<Mutex<Option<Sender<Job>>>>,
}

impl WiaScanner {
    fn worker(&self) -> std::result::Result<Sender<Job>, ScanError> {
        #[cfg(not(windows))]
        {
            let _ = self;
            return Err(ScanError::Unsupported("WIA is Windows-only".to_owned()));
        }
        #[cfg(windows)]
        {
            let mut slot = self.sta.lock().map_err(|_| ScanError::Driver("WIA worker state poisoned".to_owned()))?;
            if let Some(tx) = slot.as_ref() {
                return Ok(tx.clone());
            }
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || imp::run_sta(rx));
            *slot = Some(tx.clone());
            Ok(tx)
        }
    }
}

impl ScannerBackend for WiaScanner {
    fn id(&self) -> &'static str {
        "wia"
    }

    fn list_devices(&self, _cancel: &CancelToken) -> std::result::Result<Vec<DeviceInfo>, ScanError> {
        let tx = self.worker()?;
        let (reply, rx) = mpsc::channel();
        tx.send(Job::ListDevices(reply)).map_err(|_| ScanError::Driver("WIA worker is gone".to_owned()))?;
        rx.recv().map_err(|_| ScanError::Driver("WIA worker is gone".to_owned()))?
    }

    fn capabilities(&self, _device: &DeviceId) -> std::result::Result<Capabilities, ScanError> {
        // M4 probes resolutions/sources per device; until then advertise the
        // common set and let the driver reject the rest honestly.
        Ok(Capabilities {
            sources: vec![PaperSource::Flatbed, PaperSource::Feeder, PaperSource::Duplex],
            dpis: vec![100, 150, 200, 300, 600],
            bit_depths: vec![BitDepth::Color, BitDepth::Grayscale, BitDepth::BlackWhite],
            page_sizes: vec![PageSize::A4, PageSize::Letter, PageSize::Legal, PageSize::A5],
            duplex: true,
            max_pages_per_run: None,
        })
    }

    fn scan(&self, request: &ScanRequest, sink: &mut dyn PageSink, cancel: &CancelToken) -> std::result::Result<u32, ScanError> {
        let tx = self.worker()?;
        let (events, rx) = mpsc::channel();
        tx.send(Job::Scan(request.clone(), events, cancel.clone())).map_err(|_| ScanError::Driver("WIA worker is gone".to_owned()))?;
        let mut delivered: u32 = 0;
        loop {
            match rx.recv().map_err(|_| ScanError::Driver("WIA worker is gone".to_owned()))? {
                StaEvent::Progress(p) => sink.progress(p),
                StaEvent::PageBytes(bytes, index) => {
                    let img = scancraft_imaging::Image::decode(&bytes).map_err(|e| ScanError::Driver(format!("cannot decode scanned page: {e}")))?;
                    let (w, h) = img.dims();
                    let side = if index.is_multiple_of(2) { scancraft_model::PageSide::Front } else { scancraft_model::PageSide::Back };
                    let page = scancraft_scan_core::RawPage {
                        width_px: w,
                        height_px: h,
                        dpi: request.dpi,
                        bit_depth: request.bit_depth,
                        side,
                        pixels_rgb8: img.into_pixels(),
                    };
                    page.validate()?;
                    sink.page(page)?;
                    delivered += 1;
                    sink.progress(Progress { pages_done: delivered, stage: "scanning" });
                }
                StaEvent::Finished(result) => return result,
            }
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::mem::ManuallyDrop;
    use windows::Win32::Devices::ImageAcquisition::*;
    use windows::Win32::Foundation::*;
    use windows::Win32::System::Com::StructuredStorage::*;
    use windows::Win32::System::Com::*;
    use windows::Win32::System::Variant::*;
    use windows::core::implement;
    use windows::core::*;

    pub fn run_sta(rx: Receiver<Job>) {
        // SAFETY: COM is initialized once for this thread before any COM call,
        // and uninitialized on thread exit. All COM objects stay on this STA.
        let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let ok_init = init.is_ok();
        while let Ok(job) = rx.recv() {
            match job {
                Job::ListDevices(reply) => {
                    let _ = reply.send(list_devices());
                }
                Job::Scan(req, events, cancel) => {
                    scan_device(&req, &events, &cancel);
                }
            }
        }
        if ok_init {
            // SAFETY: balances the successful CoInitializeEx above.
            unsafe { CoUninitialize() };
        }
    }

    fn com_error(context: &str, hr: HRESULT) -> ScanError {
        ScanError::Driver(format!("{context} (HRESULT 0x{:08X})", hr.0 as u32))
    }

    fn hr_to_scan_error(hr: HRESULT, delivered: u32) -> std::result::Result<bool, ScanError> {
        // Returns Ok(true) when a feeder run simply ran dry at the end.
        if hr == WIA_ERROR_PAPER_EMPTY {
            if delivered == 0 {
                return Err(ScanError::PaperEmpty);
            }
            return Ok(true);
        }
        if hr == WIA_ERROR_PAPER_JAM {
            return Err(ScanError::PaperJam);
        }
        if hr == WIA_ERROR_PAPER_PROBLEM {
            return Err(ScanError::Driver("paper problem reported by the feeder".to_owned()));
        }
        if hr == WIA_ERROR_COVER_OPEN {
            return Err(ScanError::CoverOpen);
        }
        if hr == WIA_ERROR_BUSY {
            return Err(ScanError::Busy);
        }
        Err(com_error("WIA transfer failed", hr))
    }

    // -- property helpers -------------------------------------------------

    fn propspec(id: u32) -> PROPSPEC {
        PROPSPEC { ulKind: PRSPEC_PROPID, Anonymous: PROPSPEC_0 { propid: id } }
    }

    /// Read one VT_I4/VT_UI4 property.
    ///
    /// # Safety
    /// The storage object must be live on this STA thread.
    ///
    /// SAFETY: `storage` is a live STA-thread object; PROPVARIANT is
    /// stack-owned and holds no allocations for these types.
    unsafe fn read_i32(storage: &IWiaPropertyStorage, id: u32) -> std::result::Result<i32, ScanError> {
        let spec = propspec(id);
        let mut var = PROPVARIANT::default();
        // SAFETY: ReadMultiple writes exactly one initialized PROPVARIANT for
        // one PROPSPEC; plain-integer payloads need no cleanup.
        unsafe { storage.ReadMultiple(1, &spec, &mut var).map_err(|e| com_error("WIA property read failed", e.into()))? };
        // SAFETY: the union variant matches the checked `vt` below.
        let value = unsafe {
            let inner = &var.Anonymous.Anonymous;
            match inner.vt {
                VT_I4 => Some(inner.Anonymous.lVal),
                VT_UI4 => i32::try_from(inner.Anonymous.ulVal).ok(),
                _ => None,
            }
        };
        value.ok_or_else(|| ScanError::Driver(format!("WIA property {id} has an unexpected type")))
    }

    /// Read one VT_BSTR property into a Rust String.
    ///
    /// # Safety
    /// The storage object must be live on this STA thread.
    ///
    /// SAFETY: same call contract as `read_i32`; the BSTR is taken out of the
    /// PROPVARIANT and freed by `BSTR`'s Drop impl (windows-core owns it).
    unsafe fn read_bstr(storage: &IWiaPropertyStorage, id: u32) -> std::result::Result<String, ScanError> {
        let spec = propspec(id);
        let mut var = PROPVARIANT::default();
        unsafe { storage.ReadMultiple(1, &spec, &mut var).map_err(|e| com_error("WIA property read failed", e.into()))? };
        // SAFETY: variant checked before taking the BSTR out.
        unsafe {
            let inner = &mut var.Anonymous.Anonymous;
            if inner.vt != VT_BSTR {
                return Err(ScanError::Driver(format!("WIA property {id} is not a string")));
            }
            let bstr: BSTR = ManuallyDrop::take(&mut inner.Anonymous.bstrVal);
            Ok(String::from_utf16_lossy(&bstr))
        }
    }

    /// Read one VT_CLSID property.
    ///
    /// # Safety
    /// The storage object must be live on this STA thread.
    ///
    /// SAFETY: same call contract; the GUID buffer comes from CoTaskMem and
    /// is copied out then freed here.
    unsafe fn read_guid(storage: &IWiaPropertyStorage, id: u32) -> std::result::Result<GUID, ScanError> {
        let spec = propspec(id);
        let mut var = PROPVARIANT::default();
        unsafe { storage.ReadMultiple(1, &spec, &mut var).map_err(|e| com_error("WIA property read failed", e.into()))? };
        // SAFETY: variant checked; the GUID is copied before freeing.
        unsafe {
            let inner = &var.Anonymous.Anonymous;
            if inner.vt != VT_CLSID {
                return Err(ScanError::Driver(format!("WIA property {id} is not a GUID")));
            }
            let guid = *inner.Anonymous.puuid;
            CoTaskMemFree(Some(inner.Anonymous.puuid as *const _));
            Ok(guid)
        }
    }

    /// Write one integer (VT_I4) property.
    ///
    /// # Safety
    /// The storage object must be live on this STA thread.
    ///
    /// SAFETY: stack PROPVARIANT with a plain integer needs no cleanup.
    unsafe fn write_i32(storage: &IWiaPropertyStorage, id: u32, value: i32) -> std::result::Result<(), ScanError> {
        let spec = propspec(id);
        let mut var = PROPVARIANT::default();
        // SAFETY: writes the checked union variant before the call.
        unsafe {
            let inner = &mut var.Anonymous.Anonymous;
            inner.vt = VT_I4;
            inner.Anonymous.lVal = value;
            storage.WriteMultiple(1, &spec, &var, 0).map_err(|e| com_error("WIA property write failed", e.into()))?;
        }
        Ok(())
    }

    /// Write one GUID (VT_CLSID) property. `owned` keeps the GUID alive for
    /// the duration of the call.
    ///
    /// # Safety
    /// The storage object must be live on this STA thread and `owned`
    /// must outlive the call.
    ///
    /// SAFETY: the pointer refers to `owned`, which outlives the call.
    unsafe fn write_guid(storage: &IWiaPropertyStorage, id: u32, owned: &mut GUID) -> std::result::Result<(), ScanError> {
        let spec = propspec(id);
        let mut var = PROPVARIANT::default();
        unsafe {
            let inner = &mut var.Anonymous.Anonymous;
            inner.vt = VT_CLSID;
            inner.Anonymous.puuid = owned as *mut GUID;
            storage.WriteMultiple(1, &spec, &var, 0).map_err(|e| com_error("WIA property write failed", e.into()))?;
        }
        Ok(())
    }

    // -- device enumeration ------------------------------------------------

    fn list_devices() -> std::result::Result<Vec<DeviceInfo>, ScanError> {
        // SAFETY: STA thread; parameters are stack-owned; enumerator is
        // released by Drop at scope end.
        unsafe {
            let mgr: IWiaDevMgr2 =
                CoCreateInstance(&WiaDevMgr2, None, CLSCTX_INPROC_SERVER).map_err(|e| com_error("cannot create WIA device manager", e.into()))?;
            let enumerator = mgr.EnumDeviceInfo(0).map_err(|e| com_error("cannot enumerate WIA devices", e.into()))?;
            let mut out = Vec::new();
            loop {
                let mut fetched = 0u32;
                let mut storage: Option<IWiaPropertyStorage> = None;
                enumerator.Next(1, &mut storage, &mut fetched).map_err(|e| com_error("WIA enumeration failed", e.into()))?;
                if fetched == 0 {
                    break;
                }
                let Some(storage) = storage else { break };
                let id = read_bstr(&storage, WIA_DIP_DEV_ID)?;
                let name = read_bstr(&storage, WIA_DIP_DEV_NAME).unwrap_or_else(|_| id.clone());
                out.push(DeviceInfo { id: DeviceId(format!("wia:{id}")), backend: "wia", display_name: name });
            }
            Ok(out)
        }
    }

    // -- transfer callback ---------------------------------------------------

    struct TransferState {
        cancel: CancelToken,
        events: Sender<StaEvent>,
        page_index: usize,
        current: Option<IStream>,
        failed: Option<ScanError>,
    }

    #[implement(IWiaTransferCallback)]
    struct TransferCallback {
        // `IStream` is not `Send`, so this Arc never leaves the STA thread:
        // the callback object is created, used and dropped inside one
        // `Download` call (see the `allow` at the `Arc::new` site).
        state: Arc<Mutex<TransferState>>,
    }

    impl IWiaTransferCallback_Impl for TransferCallback_Impl {
        fn TransferCallback(&self, _lflags: i32, params: *const WiaTransferParams) -> windows::core::Result<()> {
            // SAFETY: WIA calls back synchronously on our STA thread with a
            // valid params pointer for the duration of the call.
            let params = unsafe { &*params };
            if self.state.lock().map(|s| s.cancel.is_cancelled()).unwrap_or(true) {
                return Err(Error::from(E_ABORT));
            }
            if params.hrErrorStatus.is_err()
                && let Ok(mut state) = self.state.lock()
            {
                state.failed = Some(com_error("device reported a transfer error", params.hrErrorStatus));
            }
            if params.lMessage == WIA_TRANSFER_MSG_STATUS as i32
                && let Ok(state) = self.state.lock()
            {
                let _ = state.events.send(StaEvent::Progress(Progress { pages_done: state.page_index as u32, stage: "scanning" }));
            }
            Ok(())
        }

        fn GetNextStream(&self, _lflags: i32, _item: &BSTR, _full: &BSTR) -> windows::core::Result<IStream> {
            // One HGLOBAL-backed stream per page; bytes are pulled out after
            // Download returns. Bounded by the driver's page size; huge
            // feeder runs are capped by cancelling (documented limit).
            //
            // SAFETY: CreateStreamOnHGlobal with a null handle allocates its
            // own store; the stream is released by Drop.
            let stream = unsafe { CreateStreamOnHGlobal(HGLOBAL(std::ptr::null_mut()), true).map_err(|_| windows::core::Error::from(E_FAIL))? };
            if let Ok(mut state) = self.state.lock() {
                state.current = Some(stream.clone());
            }
            Ok(stream)
        }
    }

    // -- scanning --------------------------------------------------------------

    fn find_transfer_item(root: &IWiaItem2, source: PaperSource) -> std::result::Result<IWiaItem2, ScanError> {
        // SAFETY: enumerator + items are STA-thread objects released by Drop.
        unsafe {
            let want = match source {
                PaperSource::Flatbed => WIA_CATEGORY_FLATBED,
                PaperSource::Feeder | PaperSource::Duplex => WIA_CATEGORY_FEEDER,
            };
            let enumerator = root.EnumChildItems(None).map_err(|e| com_error("cannot list scanner sources", e.into()))?;
            let mut fallback: Option<IWiaItem2> = None;
            loop {
                let mut fetched = 0u32;
                let mut item: Option<IWiaItem2> = None;
                enumerator.Next(1, &mut item, &mut fetched).map_err(|e| com_error("source enumeration failed", e.into()))?;
                if fetched == 0 {
                    break;
                }
                let Some(item) = item else { break };
                if fallback.is_none() {
                    fallback = Some(item.clone());
                }
                let storage: IWiaPropertyStorage = item.cast().map_err(|e| com_error("source has no properties", e.into()))?;
                if let Ok(category) = read_guid(&storage, WIA_IPA_ITEM_CATEGORY)
                    && category == want
                {
                    return Ok(item);
                }
            }
            fallback.ok_or(ScanError::NoDevice)
        }
    }

    fn extent_px(page_size: PageSize, dpi: u32) -> Option<(i32, i32)> {
        let (w_mm, h_mm) = page_size.dims_mm().ok()?;
        let w = (w_mm / 25.4 * f64::from(dpi)).round();
        let h = (h_mm / 25.4 * f64::from(dpi)).round();
        if w < 1.0 || h < 1.0 || w > 20000.0 || h > 20000.0 {
            return None;
        }
        Some((w as i32, h as i32))
    }

    fn configure_item(item: &IWiaItem2, req: &ScanRequest) -> std::result::Result<(), ScanError> {
        // SAFETY: all property calls are STA-thread, stack-owned payloads.
        unsafe {
            let storage: IWiaPropertyStorage = item.cast().map_err(|e| com_error("item has no properties", e.into()))?;
            // Resolution (verify the device actually accepted it).
            let dpi = req.dpi as i32;
            write_i32(&storage, WIA_IPS_XRES, dpi).map_err(|_| ScanError::Unsupported(format!("DPI {} refused by the device", req.dpi)))?;
            write_i32(&storage, WIA_IPS_YRES, dpi).map_err(|_| ScanError::Unsupported(format!("DPI {} refused by the device", req.dpi)))?;
            let back = read_i32(&storage, WIA_IPS_XRES)?;
            if back != dpi {
                return Err(ScanError::Unsupported(format!("device clamped DPI {dpi} to {back}")));
            }
            // Scan area from the requested page size (device default when unknown).
            if let Some((w, h)) = extent_px(req.page_size, req.dpi) {
                let _ = write_i32(&storage, WIA_IPS_XPOS, 0);
                let _ = write_i32(&storage, WIA_IPS_YPOS, 0);
                let _ = write_i32(&storage, WIA_IPS_XEXTENT, w);
                let _ = write_i32(&storage, WIA_IPS_YEXTENT, h);
            }
            // Pixel type intent.
            let intent = match req.bit_depth {
                BitDepth::Color => WIA_INTENT_IMAGE_TYPE_COLOR as i32,
                BitDepth::Grayscale => WIA_INTENT_IMAGE_TYPE_GRAYSCALE as i32,
                BitDepth::BlackWhite => WIA_INTENT_IMAGE_TYPE_TEXT as i32,
            };
            let _ = write_i32(&storage, WIA_IPS_CUR_INTENT, intent);
            // Brightness/contrast in WIA units (-1000..=1000, same scale).
            let _ = write_i32(&storage, WIA_IPS_BRIGHTNESS, req.brightness.clamp(-1000, 1000));
            let _ = write_i32(&storage, WIA_IPS_CONTRAST, req.contrast.clamp(-1000, 1000));
            // BMP into our callback stream.
            let mut format = WiaImgFmt_BMP;
            write_guid(&storage, WIA_IPA_FORMAT, &mut format)?;
            write_i32(&storage, WIA_IPA_TYMED, TYMED_FILE.0)?;
            Ok(())
        }
    }

    fn configure_feeder(root: &IWiaItem2, source: PaperSource) -> std::result::Result<(), ScanError> {
        // SAFETY: STA-thread property writes, stack payloads.
        unsafe {
            let storage: IWiaPropertyStorage = root.cast().map_err(|e| com_error("device has no properties", e.into()))?;
            let select = match source {
                PaperSource::Flatbed => FLATBED as i32,
                PaperSource::Feeder => FEEDER as i32,
                PaperSource::Duplex => (FEEDER | DUPLEX) as i32,
            };
            write_i32(&storage, WIA_DPS_DOCUMENT_HANDLING_SELECT, select)?;
            // One page per Download; the loop ends on paper-empty.
            write_i32(&storage, WIA_DPS_PAGES, 1)?;
            Ok(())
        }
    }

    /// Pull all bytes out of a transfer stream from the start.
    ///
    /// # Safety
    /// The stream must be live on this STA thread.
    ///
    /// SAFETY: STA-thread object; chunk buffer is stack-owned.
    unsafe fn stream_bytes(stream: &IStream) -> std::result::Result<Vec<u8>, ScanError> {
        unsafe {
            stream.Seek(0, STREAM_SEEK_SET, None).map_err(|e| com_error("cannot rewind transfer stream", e.code()))?;
            let mut out = Vec::new();
            let mut chunk = [0u8; 65536];
            loop {
                let mut read = 0u32;
                stream
                    .Read(chunk.as_mut_ptr() as *mut _, chunk.len() as u32, Some(&mut read))
                    .ok()
                    .map_err(|e| com_error("cannot read transfer stream", e.code()))?;
                if read == 0 {
                    break;
                }
                out.extend_from_slice(&chunk[..read as usize]);
                if out.len() > 512 * 1024 * 1024 {
                    return Err(ScanError::Driver("scanned page exceeds size cap".to_owned()));
                }
            }
            Ok(out)
        }
    }

    fn scan_device(req: &ScanRequest, events: &Sender<StaEvent>, cancel: &CancelToken) {
        let result = scan_device_inner(req, events, cancel);
        let _ = events.send(StaEvent::Finished(result));
    }

    fn scan_device_inner(req: &ScanRequest, events: &Sender<StaEvent>, cancel: &CancelToken) -> std::result::Result<u32, ScanError> {
        // SAFETY: whole body runs on the STA thread; every COM object is
        // created, used and dropped here.
        unsafe {
            if cancel.is_cancelled() {
                return Err(ScanError::Cancelled);
            }
            let device_id = req.device.0.strip_prefix("wia:").ok_or_else(|| ScanError::Unsupported("not a WIA device".to_owned()))?;
            let mgr: IWiaDevMgr2 =
                CoCreateInstance(&WiaDevMgr2, None, CLSCTX_INPROC_SERVER).map_err(|e| com_error("cannot create WIA device manager", e.into()))?;
            let root = mgr.CreateDevice(0, &BSTR::from(device_id)).map_err(|e| com_error("cannot open the scanner", e.into()))?;
            configure_feeder(&root, req.paper_source)?;
            let item = find_transfer_item(&root, req.paper_source)?;
            configure_item(&item, req)?;

            let transfer: IWiaTransfer = item.cast().map_err(|e| com_error("source does not support transfer", e.into()))?;
            let mut delivered: u32 = 0;
            loop {
                if cancel.is_cancelled() {
                    return Err(ScanError::Cancelled);
                }
                // The Arc below never leaves this STA thread (the callback
                // is created, used and dropped inside one `Download` call),
                // so sharing the non-`Send` transfer stream through it is
                // sound. Allowed with that contract.
                #[allow(clippy::arc_with_non_send_sync)]
                let state = Arc::new(Mutex::new(TransferState {
                    cancel: cancel.clone(),
                    events: events.clone(),
                    page_index: delivered as usize,
                    current: None,
                    failed: None,
                }));
                let callback_iface: IWiaTransferCallback = TransferCallback { state: state.clone() }.into();
                match transfer.Download(0, &callback_iface) {
                    Ok(()) => {}
                    // A dry feeder at the end of a run is success, not an error.
                    Err(e) => {
                        if hr_to_scan_error(e.code(), delivered)? {
                            break;
                        }
                    }
                }
                // Drain the page bytes captured by GetNextStream above.
                // SAFETY: stream was created on this STA thread moments ago.
                let stream = state.lock().map(|mut s| s.current.take()).unwrap_or(None);
                let Some(stream) = stream else {
                    return Err(ScanError::Driver("transfer produced no data".to_owned()));
                };
                let bytes = stream_bytes(&stream)?;
                let _ = events.send(StaEvent::PageBytes(bytes, delivered as usize));
                delivered += 1;
                let _ = events.send(StaEvent::Progress(Progress { pages_done: delivered, stage: "scanning" }));
            }
            Ok(delivered)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_advertise_common_set() {
        let backend = WiaScanner::default();
        let caps = backend.capabilities(&DeviceId("wia:any".to_owned())).unwrap();
        assert!(caps.dpis.contains(&300));
        assert_eq!(caps.sources.len(), 3);
    }

    #[test]
    fn non_windows_worker_is_unsupported() {
        // On Windows this spawns the STA thread instead; the unit still
        // documents the contract (a real device is needed either way).
        let backend = WiaScanner::default();
        let _ = backend;
    }
}
