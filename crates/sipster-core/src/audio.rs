use crate::error::{Result, SipsterError};
use cpal::traits::{DeviceTrait, HostTrait};
use tracing::info;

/// Audio engine abstraction using cpal and audio-codec
pub struct AudioEngine {
    _host: cpal::Host,
}

impl AudioEngine {
    pub fn new() -> Result<Self> {
        let host = cpal::default_host();
        info!("Initialized audio engine with host: {:?}", host.id());
        Ok(Self { _host: host })
    }

    pub fn list_input_devices(&self) -> Result<Vec<String>> {
        let host = cpal::default_host();
        let devices = host
            .input_devices()
            .map_err(|e| SipsterError::Audio(e.to_string()))?;
        
        let mut names = Vec::new();
        for dev in devices {
            if let Ok(name) = dev.name() {
                names.push(name);
            }
        }
        Ok(names)
    }

    pub fn list_output_devices(&self) -> Result<Vec<String>> {
        let host = cpal::default_host();
        let devices = host
            .output_devices()
            .map_err(|e| SipsterError::Audio(e.to_string()))?;
        
        let mut names = Vec::new();
        for dev in devices {
            if let Ok(name) = dev.name() {
                names.push(name);
            }
        }
        Ok(names)
    }
}
