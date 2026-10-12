//! Operator diagnostic: enumerate capture endpoints and report, for each one,
//! the mix format the driver advertises and which formats it will actually
//! accept. Run with `cargo run -p audio-capture --example probe`.
//!
//! This exists because "the driver refused the format" is the single most
//! common real-world failure when bringing up a new audio interface, and the
//! only way to debug it is to see what the driver actually said.

use audio_capture::wasapi::{
    ComApartment, activate_client, build_format, device_periods, mix_format, open_device,
    share_mode_flag,
};
use audio_core::{AudioFormat, SampleFormat, ShareMode};
use windows::Win32::Media::Audio::WAVEFORMATEX;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let devices = audio_capture::enumerate_devices(true)?;
    println!("found {} capture endpoint(s)\n", devices.len());

    let _apartment = ComApartment::enter()?;

    for device in &devices {
        println!("── {} ", device.name);
        println!("   id            {}", device.id);
        println!("   state         {}", device.state.as_str());
        println!("   default       {}", device.is_default);
        println!("   channels      {}", device.input_channels);
        println!(
            "   mix format    {} Hz {}",
            device.default_sample_rate,
            device.default_sample_format.as_str()
        );
        println!("   probed rates  {:?}", device.supported_sample_rates);
        println!(
            "   period frames default={:?} min={:?}",
            device.default_period_frames, device.minimum_period_frames
        );

        if !device.state.is_usable() {
            println!("   (not active; skipping format probe)\n");
            continue;
        }

        let Ok(immdevice) = open_device(&device.id) else {
            println!("   ! could not open endpoint\n");
            continue;
        };
        let Ok(client) = activate_client(&immdevice) else {
            println!("   ! could not activate IAudioClient\n");
            continue;
        };

        match mix_format(&client) {
            Ok(format) => println!(
                "   parsed mix    {} Hz, {} ch, {}",
                format.sample_rate,
                format.channels,
                format.sample_format.as_str()
            ),
            Err(error) => println!("   ! mix format unreadable: {error}"),
        }

        let (default_period, minimum_period) = device_periods(&client, device.default_sample_rate);
        println!("   periods       default={default_period:?} min={minimum_period:?}");

        println!("   IsFormatSupported (shared):");
        for rate in [44_100_u32, 48_000, 96_000] {
            for channels in [1_u16, device.input_channels] {
                for sample_format in [SampleFormat::F32, SampleFormat::I16, SampleFormat::I32] {
                    let candidate = AudioFormat::new(rate, channels, sample_format);
                    let wave = build_format(&candidate);
                    let mut closest: *mut WAVEFORMATEX = core::ptr::null_mut();
                    // SAFETY: shared mode requires a non-null closest-match
                    // out-pointer; we free whatever it hands back.
                    let hresult = unsafe {
                        let hr = client.IsFormatSupported(
                            share_mode_flag(ShareMode::Shared),
                            core::ptr::from_ref(&wave).cast::<WAVEFORMATEX>(),
                            Some(&mut closest),
                        );
                        if !closest.is_null() {
                            windows::Win32::System::Com::CoTaskMemFree(Some(closest.cast()));
                        }
                        hr
                    };
                    println!(
                        "     {rate:>6} Hz {channels} ch {:<4} -> 0x{:08X} {}",
                        sample_format.as_str(),
                        hresult.0,
                        if hresult == windows::Win32::Foundation::S_OK { "OK" } else { "" }
                    );
                }
            }
        }
        println!();
    }

    Ok(())
}
