//! HEIC decoding through Windows' own image decoders (the Windows Imaging
//! Component). The app bundles no HEIC decoder: it uses the one Windows
//! provides when the HEIF and HEVC extensions are installed, as they are on
//! most PCs. Nothing here goes online.

use crate::rendered::Pixels;

#[cfg(not(windows))]
pub(crate) fn decode(_bytes: &[u8]) -> Result<Pixels, String> {
    Err("HEIC is only supported on Windows".into())
}

#[cfg(windows)]
pub(crate) fn decode(bytes: &[u8]) -> Result<Pixels, String> {
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, GUID_WICPixelFormat32bppRGBA, IWICBitmapSource, IWICImagingFactory, WICBitmapDitherTypeNone,
        WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};

    let fail = |what: &str, e: windows::core::Error| format!("{what}: {}", e.message());
    // SAFETY: plain COM calls on objects created and released within this
    // function; every buffer passed is sized from the values WIC reports.
    unsafe {
        // Fails harmlessly if this thread already uses COM in another mode.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).map_err(|e| fail("Windows imaging isn't available", e))?;
        let stream = factory.CreateStream().map_err(|e| fail("stream", e))?;
        stream.InitializeFromMemory(bytes).map_err(|e| fail("stream", e))?;
        let decoder = factory
            .CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)
            .map_err(|e| fail("no decoder for this file", e))?;
        let frame = decoder.GetFrame(0).map_err(|e| fail("no picture in the file", e))?;

        let (mut w, mut h) = (0u32, 0u32);
        frame.GetSize(&mut w, &mut h).map_err(|e| fail("size", e))?;
        if w == 0 || h == 0 || (w as u64) * (h as u64) > 200_000_000 {
            return Err(format!("unsupported size {w}×{h}"));
        }

        // The colour profile, to tell Display P3 from sRGB.
        let mut icc = None;
        if let Ok(context) = factory.CreateColorContext() {
            let mut slots = [Some(context)];
            let mut found = 0u32;
            if frame.GetColorContexts(&mut slots, &mut found).is_ok() && found > 0 {
                if let Some(context) = &slots[0] {
                    // Profiles are a few kilobytes at most; one generous buffer avoids asking twice.
                    let mut len = 0u32;
                    let mut profile = vec![0u8; 1 << 20];
                    if context.GetProfileBytes(&mut profile, &mut len).is_ok() && len > 0 && (len as usize) <= profile.len() {
                        profile.truncate(len as usize);
                        icc = Some(profile);
                    }
                }
            }
        }

        // Windows' HEIF decoder hands back 8 bits per colour.
        let converter = factory.CreateFormatConverter().map_err(|e| fail("converter", e))?;
        converter
            .Initialize(&frame, &GUID_WICPixelFormat32bppRGBA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)
            .map_err(|e| fail("this picture can't be converted", e))?;
        let source: IWICBitmapSource = windows::core::Interface::cast(&converter).map_err(|e| fail("converter", e))?;
        let mut raw = vec![0u8; w as usize * h as usize * 4];
        source.CopyPixels(std::ptr::null(), w * 4, &mut raw).map_err(|e| fail("decoding failed", e))?;
        // 0…255 becomes 0…65535 exactly (255 × 257 = 65535).
        let rgba: Vec<u16> = raw.iter().map(|v| *v as u16 * 257).collect();
        let bits = 8;
        // Windows' HEIF decoder already turns the picture upright.
        Ok(Pixels { width: w as usize, height: h as usize, rgba, bits, icc, orientation: 1 })
    }
}
