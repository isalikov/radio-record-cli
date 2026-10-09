//! Read the hardware clock without changing device properties. CPAL's default
//! config reports the virtual stream format, which can differ during a call.
use objc2_core_audio::{
    AudioObjectGetPropertyData, AudioObjectPropertyAddress, kAudioDevicePropertyNominalSampleRate,
    kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyElementMain,
    kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject,
};
use std::{
    mem::size_of,
    ptr::{NonNull, null},
};

pub(super) fn default_output_rate() -> Option<u32> {
    let mut address = AudioObjectPropertyAddress {
        mSelector: kAudioHardwarePropertyDefaultOutputDevice,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut device_id = 0u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: address, size, and the correctly sized output buffer stay alive
    // throughout this synchronous read; there are no qualifier bytes.
    let status = unsafe {
        AudioObjectGetPropertyData(
            kAudioObjectSystemObject as u32,
            NonNull::from(&address),
            0,
            null(),
            NonNull::from(&mut size),
            NonNull::from(&mut device_id).cast(),
        )
    };
    if status != 0 || device_id == 0 || size != size_of::<u32>() as u32 {
        return None;
    }
    address.mSelector = kAudioDevicePropertyNominalSampleRate;
    let mut rate = 0.0f64;
    size = size_of::<f64>() as u32;
    // SAFETY: same contract, now using the documented Float64 property buffer.
    let status = unsafe {
        AudioObjectGetPropertyData(
            device_id,
            NonNull::from(&address),
            0,
            null(),
            NonNull::from(&mut size),
            NonNull::from(&mut rate).cast(),
        )
    };
    if status != 0
        || size != size_of::<f64>() as u32
        || !rate.is_finite()
        || rate < 1.0
        || rate > f64::from(u32::MAX)
    {
        return None;
    }
    Some(rate.round() as u32)
}
