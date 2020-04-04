pub mod constants;
pub mod memory_interface;
pub mod tools;
mod usb_interface;

use self::usb_interface::STLinkUSBDevice;
use super::{
    DAPAccess, DebugProbe, DebugProbeError, DebugProbeInfo, JTAGAccess, PortType, WireProtocol,
};
use crate::Memory;
use constants::{commands, JTagFrequencyToDivider, Mode, Status, SwdFrequencyToDelayCount};
use scroll::{Pread, BE, LE};
use thiserror::Error;
use usb_interface::TIMEOUT;
use num_traits::cast::FromPrimitive;

#[derive(Debug)]
pub struct STLink {
    device: STLinkUSBDevice,
    hw_version: u8,
    jtag_version: u8,
    protocol: WireProtocol,
    swd_speed_khz: u32,
    jtag_speed_khz: u32,

    /// Index of the AP which is currently open.
    current_ap: Option<u16>,
}

impl DebugProbe for STLink {
    fn new_from_probe_info(info: &DebugProbeInfo) -> Result<Box<Self>, DebugProbeError> {
        let mut stlink = Self {
            device: STLinkUSBDevice::new_from_info(info)?,
            hw_version: 0,
            jtag_version: 0,
            protocol: WireProtocol::Swd,
            swd_speed_khz: 1_800,
            jtag_speed_khz: 1_120,

            current_ap: None,
        };

        stlink.init()?;

        Ok(Box::new(stlink))
    }

    fn get_name(&self) -> &str {
        "ST-Link"
    }

    fn speed(&self) -> u32 {
        match self.protocol {
            WireProtocol::Swd => self.swd_speed_khz,
            WireProtocol::Jtag => self.jtag_speed_khz,
        }
    }

    fn set_speed(&mut self, speed_khz: u32) -> Result<u32, DebugProbeError> {
        if self.hw_version < 3 {
            match self.protocol {
                WireProtocol::Swd => {
                    let actual_speed = SwdFrequencyToDelayCount::find_setting(speed_khz);

                    if let Some(actual_speed) = actual_speed {
                        self.set_swd_frequency(actual_speed)?;

                        self.swd_speed_khz = actual_speed.to_khz();

                        Ok(actual_speed.to_khz())
                    } else {
                        Err(DebugProbeError::UnsupportedSpeed(speed_khz))
                    }
                }
                WireProtocol::Jtag => {
                    let actual_speed = JTagFrequencyToDivider::find_setting(speed_khz);

                    if let Some(actual_speed) = actual_speed {
                        self.set_jtag_frequency(actual_speed)?;

                        self.jtag_speed_khz = actual_speed.to_khz();

                        Ok(actual_speed.to_khz())
                    } else {
                        Err(DebugProbeError::UnsupportedSpeed(speed_khz))
                    }
                }
            }
        } else if self.hw_version == 3 {
            let (available, _) = self.get_communication_frequencies(self.protocol)?;

            let actual_speed_khz = available
                .into_iter()
                .filter(|speed| *speed <= speed_khz)
                .max()
                .ok_or(DebugProbeError::UnsupportedSpeed(speed_khz))?;

            self.set_communication_frequency(self.protocol, actual_speed_khz)?;

            match self.protocol {
                WireProtocol::Swd => self.swd_speed_khz = actual_speed_khz,
                WireProtocol::Jtag => self.jtag_speed_khz = actual_speed_khz,
            }

            Ok(actual_speed_khz)
        } else {
            unimplemented!()
        }
    }

    /// Enters debug mode.
    fn attach(&mut self) -> Result<(), DebugProbeError> {
        log::debug!("attach({:?})", self.protocol);
        self.enter_idle()?;

        let param = match self.protocol {
            WireProtocol::Jtag => {
                log::debug!("Switching protocol to JTAG");
                commands::JTAG_ENTER_JTAG_NO_CORE_RESET
            }
            WireProtocol::Swd => {
                log::debug!("Switching protocol to SWD");
                commands::JTAG_ENTER_SWD
            }
        };

        let mut buf = [0; 2];
        self.device.write(
            vec![commands::JTAG_COMMAND, commands::JTAG_ENTER2, param, 0],
            &[],
            &mut buf,
            TIMEOUT,
        )?;
        Self::check_status(&buf)?;

        log::debug!("Successfully initialized SWD.");

        // If the speed is not manually set, the probe will
        // use whatever speed has been configured before.
        //
        // To ensure the default speed is used if not changed,
        // we set the speed again here.
        match self.protocol {
            WireProtocol::Jtag => {
                self.set_speed(self.jtag_speed_khz)?;
            }
            WireProtocol::Swd => {
                self.set_speed(self.swd_speed_khz)?;
            }
        }

        Ok(())
    }

    /// Leave debug mode.
    fn detach(&mut self) -> Result<(), DebugProbeError> {
        log::debug!("Detaching from STLink.");
        self.enter_idle()
    }

    /// Asserts the nRESET pin.
    fn target_reset(&mut self) -> Result<(), DebugProbeError> {
        let mut buf = [0; 2];
        self.device.write(
            vec![
                commands::JTAG_COMMAND,
                commands::JTAG_DRIVE_NRST,
                commands::JTAG_DRIVE_NRST_PULSE,
            ],
            &[],
            &mut buf,
            TIMEOUT,
        )?;

        Self::check_status(&buf)
    }

    fn select_protocol(&mut self, protocol: WireProtocol) -> Result<(), DebugProbeError> {
        match protocol {
            WireProtocol::Jtag => self.protocol = WireProtocol::Jtag,
            WireProtocol::Swd => self.protocol = WireProtocol::Swd,
        }
        Ok(())
    }

    fn dedicated_memory_interface(&self) -> Option<Memory> {
        None
    }

    fn get_interface_dap(&self) -> Option<&dyn DAPAccess> {
        Some(self as _)
    }

    fn get_interface_dap_mut(&mut self) -> Option<&mut dyn DAPAccess> {
        Some(self as _)
    }

    fn get_interface_jtag(&self) -> Option<&dyn JTAGAccess> {
        None
    }

    fn get_interface_jtag_mut(&mut self) -> Option<&mut dyn JTAGAccess> {
        None
    }
}

impl DAPAccess for STLink {
    /// Reads the DAP register on the specified port and address.
    fn read_register(&mut self, port: PortType, addr: u16) -> Result<u32, DebugProbeError> {
        if (addr & 0xf0) == 0 || port != PortType::DebugPort {
            if let PortType::AccessPort(port_number) = port {
                if let Some(current_ap) = self.current_ap {
                    if current_ap != port_number {
                        self.close_ap(current_ap as u8)?;
                        self.open_ap(port_number as u8)?;
                    }
                } else {
                    // First time reading, open the AP
                    self.open_ap(port_number as u8)?;
                }

                self.current_ap = Some(port_number);
            }

            let port: u16 = port.into();

            let cmd = vec![
                commands::JTAG_COMMAND,
                commands::JTAG_READ_DAP_REG,
                (port & 0xFF) as u8,
                ((port >> 8) & 0xFF) as u8,
                (addr & 0xFF) as u8,
                ((addr >> 8) & 0xFF) as u8,
            ];
            let mut buf = [0; 8];
            self.device.write(cmd, &[], &mut buf, TIMEOUT)?;
            Self::check_status(&buf)?;
            // Unwrap is ok!
            Ok((&buf[4..8]).pread_with(0, LE).unwrap())
        } else {
            Err(StlinkError::BlanksNotAllowedOnDPRegister.into())
        }
    }

    /// Writes a value to the DAP register on the specified port and address.
    fn write_register(
        &mut self,
        port: PortType,
        addr: u16,
        value: u32,
    ) -> Result<(), DebugProbeError> {
        if (addr & 0xf0) == 0 || port != PortType::DebugPort {
            if let PortType::AccessPort(port_number) = port {
                if let Some(current_ap) = self.current_ap {
                    if current_ap != port_number {
                        self.close_ap(current_ap as u8)?;
                        self.open_ap(port_number as u8)?;
                    }
                } else {
                    // First time reading, open the AP
                    self.open_ap(port_number as u8)?;
                }

                self.current_ap = Some(port_number);
            }

            let port: u16 = port.into();

            let cmd = vec![
                commands::JTAG_COMMAND,
                commands::JTAG_WRITE_DAP_REG,
                (port & 0xFF) as u8,
                ((port >> 8) & 0xFF) as u8,
                (addr & 0xFF) as u8,
                ((addr >> 8) & 0xFF) as u8,
                (value & 0xFF) as u8,
                ((value >> 8) & 0xFF) as u8,
                ((value >> 16) & 0xFF) as u8,
                ((value >> 24) & 0xFF) as u8,
            ];
            let mut buf = [0; 2];
            self.device.write(cmd, &[], &mut buf, TIMEOUT)?;
            Self::check_status(&buf)?;
            Ok(())
        } else {
            Err(StlinkError::BlanksNotAllowedOnDPRegister.into())
        }
    }
}

impl Drop for STLink {
    fn drop(&mut self) {
        // We ignore the error case as we can't do much about it anyways.
        let _ = self.enter_idle();
    }
}

impl STLink {
    /// Maximum number of bytes to send or receive for 32- and 16- bit transfers.
    ///
    /// 8-bit transfers have a maximum size of the maximum USB packet size (64 bytes for full speed).
    const _MAXIMUM_TRANSFER_SIZE: u32 = 1024;

    /// Minimum required STLink firmware version.
    const MIN_JTAG_VERSION: u8 = 24;

    /// Firmware version that adds 16-bit transfers.
    const _MIN_JTAG_VERSION_16BIT_XFER: u8 = 26;

    /// Firmware version that adds multiple AP support.
    const MIN_JTAG_VERSION_MULTI_AP: u8 = 28;

    /// Reads the target voltage.
    /// For the china fake variants this will always read a nonzero value!
    pub fn get_target_voltage(&mut self) -> Result<f32, DebugProbeError> {
        let mut buf = [0; 8];
        match self
            .device
            .write(vec![commands::GET_TARGET_VOLTAGE], &[], &mut buf, TIMEOUT)
        {
            Ok(_) => {
                // The next two unwraps are safe!
                let a0 = (&buf[0..4]).pread_with::<u32>(0, LE).unwrap() as f32;
                let a1 = (&buf[4..8]).pread_with::<u32>(0, LE).unwrap() as f32;
                if a0 != 0.0 {
                    Ok((2.0 * a1 * 1.2 / a0) as f32)
                } else {
                    // Should never happen
                    Err(StlinkError::VoltageDivisionByZero.into())
                }
            }
            Err(e) => Err(e),
        }
    }

    /// Get the current mode of the ST-Link
    fn get_current_mode(&mut self) -> Result<Mode, DebugProbeError> {
        log::trace!("Getting current mode of device...");
        let mut buf = [0; 2];
        self.device
            .write(vec![commands::GET_CURRENT_MODE], &[], &mut buf, TIMEOUT)?;

        use Mode::*;

        let mode = match buf[0] {
            0 => Dfu,
            1 => MassStorage,
            2 => Jtag,
            3 => Swim,
            _ => return Err(StlinkError::UnknownMode.into()),
        };

        log::debug!("Current device mode: {:?}", mode);

        Ok(mode)
    }

    /// Commands the ST-Link to enter idle mode.
    /// Internal helper.
    fn enter_idle(&mut self) -> Result<(), DebugProbeError> {
        let mode = self.get_current_mode()?;

        match mode {
            Mode::Dfu => self.device.write(
                vec![commands::DFU_COMMAND, commands::DFU_EXIT],
                &[],
                &mut [],
                TIMEOUT,
            ),
            Mode::Swim => self.device.write(
                vec![commands::SWIM_COMMAND, commands::SWIM_EXIT],
                &[],
                &mut [],
                TIMEOUT,
            ),
            _ => Ok(()),
        }
    }

    /// Reads the ST-Links version.
    /// Returns a tuple (hardware version, firmware version).
    /// This method stores the version data on the struct to make later use of it.
    fn get_version(&mut self) -> Result<(u8, u8), DebugProbeError> {
        const HW_VERSION_SHIFT: u8 = 12;
        const HW_VERSION_MASK: u8 = 0x0F;
        const JTAG_VERSION_SHIFT: u8 = 6;
        const JTAG_VERSION_MASK: u8 = 0x3F;
        // GET_VERSION response structure:
        //   Byte 0-1:
        //     [15:12] Major/HW version
        //     [11:6]  JTAG/SWD version
        //     [5:0]   SWIM or MSC version
        //   Byte 2-3: ST_VID
        //   Byte 4-5: STLINK_PID
        let mut buf = [0; 6];
        match self
            .device
            .write(vec![commands::GET_VERSION], &[], &mut buf, TIMEOUT)
        {
            Ok(_) => {
                let version: u16 = (&buf[0..2]).pread_with(0, BE).unwrap();
                self.hw_version = (version >> HW_VERSION_SHIFT) as u8 & HW_VERSION_MASK;
                self.jtag_version = (version >> JTAG_VERSION_SHIFT) as u8 & JTAG_VERSION_MASK;
            }
            Err(e) => return Err(e),
        }

        // For the STLinkV3 we must use the extended get version command.
        if self.hw_version >= 3 {
            // GET_VERSION_EXT response structure (byte offsets)
            //  0: HW version
            //  1: SWIM version
            //  2: JTAG/SWD version
            //  3: MSC/VCP version
            //  4: Bridge version
            //  5-7: reserved
            //  8-9: ST_VID
            //  10-11: STLINK_PID
            let mut buf = [0; 12];
            match self
                .device
                .write(vec![commands::GET_VERSION_EXT], &[], &mut buf, TIMEOUT)
            {
                Ok(_) => {
                    let version: u8 = (&buf[2..3]).pread_with(0, LE).unwrap();
                    self.jtag_version = version;
                }
                Err(e) => return Err(e),
            }
        }

        // Make sure everything is okay with the firmware we use.
        if self.jtag_version == 0 {
            return Err(DebugProbeError::JTAGNotSupportedOnProbe);
        }
        if self.hw_version < 3 && self.jtag_version < Self::MIN_JTAG_VERSION {
            return Err(DebugProbeError::ProbeFirmwareOutdated);
        }

        Ok((self.hw_version, self.jtag_version))
    }

    /// Opens the ST-Link USB device and tries to identify the ST-Links version and it's target voltage.
    /// Internal helper.
    fn init(&mut self) -> Result<(), DebugProbeError> {
        log::debug!("Initializing STLink...");

        if let Err(e) = self.enter_idle() {
            match e {
                DebugProbeError::USB(_) => {
                    // Reset the device, and try to enter idle mode again
                    self.device.reset()?;

                    self.enter_idle()?;
                }
                // Other error occured, return it
                _ => return Err(e),
            }
        }

        let version = self.get_version()?;
        log::debug!("STLink version: {:?}", version);

        if self.hw_version == 3 {
            let (_, current) = self.get_communication_frequencies(WireProtocol::Swd)?;
            self.swd_speed_khz = current;

            let (_, current) = self.get_communication_frequencies(WireProtocol::Jtag)?;
            self.jtag_speed_khz = current;
        }

        self.get_target_voltage().map(|_| ())
    }

    /// sets the SWD frequency.
    pub fn set_swd_frequency(
        &mut self,
        frequency: SwdFrequencyToDelayCount,
    ) -> Result<(), DebugProbeError> {
        let mut buf = [0; 2];
        self.device.write(
            vec![
                commands::JTAG_COMMAND,
                commands::SWD_SET_FREQ,
                frequency as u8,
            ],
            &[],
            &mut buf,
            TIMEOUT,
        )?;
        Self::check_status(&buf)
    }

    /// Sets the JTAG frequency.
    pub fn set_jtag_frequency(
        &mut self,
        frequency: JTagFrequencyToDivider,
    ) -> Result<(), DebugProbeError> {
        let mut buf = [0; 2];
        self.device.write(
            vec![
                commands::JTAG_COMMAND,
                commands::JTAG_SET_FREQ,
                frequency as u8,
            ],
            &[],
            &mut buf,
            TIMEOUT,
        )?;
        Self::check_status(&buf)
    }

    /// Sets the communication frequency (V3 only)
    fn set_communication_frequency(
        &mut self,
        protocol: WireProtocol,
        frequency_khz: u32,
    ) -> Result<(), DebugProbeError> {
        assert_eq!(self.hw_version, 3);

        let cmd_proto = match protocol {
            WireProtocol::Swd => 0,
            WireProtocol::Jtag => 1,
        };

        let mut command = vec![commands::JTAG_COMMAND, commands::SET_COM_FREQ, cmd_proto, 0];
        command.extend_from_slice(&frequency_khz.to_le_bytes());

        let mut buf = [0; 8];
        self.device.write(command, &[], &mut buf, TIMEOUT)?;
        Self::check_status(&buf)
    }

    /// Returns the current and available communication frequencies (V3 only)
    fn get_communication_frequencies(
        &mut self,
        protocol: WireProtocol,
    ) -> Result<(Vec<u32>, u32), DebugProbeError> {
        assert_eq!(self.hw_version, 3);

        let cmd_proto = match protocol {
            WireProtocol::Swd => 0,
            WireProtocol::Jtag => 1,
        };

        let mut buf = [0; 52];
        self.device.write(
            vec![commands::JTAG_COMMAND, commands::GET_COM_FREQ, cmd_proto],
            &[],
            &mut buf,
            TIMEOUT,
        )?;
        Self::check_status(&buf)?;

        let mut values = (&buf)
            .chunks(4)
            .map(|chunk| chunk.pread_with::<u32>(0, LE).unwrap())
            .collect::<Vec<u32>>();

        let current = values[1];
        let n = core::cmp::min(values[2], 10) as usize;

        values.rotate_left(3);
        values.truncate(n);

        Ok((values, current))
    }

    pub fn open_ap(&mut self, apsel: u8) -> Result<(), DebugProbeError> {
        if self.hw_version < 3 && self.jtag_version < Self::MIN_JTAG_VERSION_MULTI_AP {
            Err(StlinkError::JTagDoesNotSupportMultipleAP.into())
        } else {
            let mut buf = [0; 2];
            log::trace!("JTAG_INIT_AP {}", apsel);
            self.device.write(
                vec![commands::JTAG_COMMAND, commands::JTAG_INIT_AP, apsel],
                &[],
                &mut buf,
                TIMEOUT,
            )?;
            Self::check_status(&buf)
        }
    }

    pub fn close_ap(&mut self, apsel: u8) -> Result<(), DebugProbeError> {
        if self.hw_version < 3 && self.jtag_version < Self::MIN_JTAG_VERSION_MULTI_AP {
            Err(StlinkError::JTagDoesNotSupportMultipleAP.into())
        } else {
            let mut buf = [0; 2];
            log::trace!("JTAG_CLOSE_AP {}", apsel);
            self.device.write(
                vec![commands::JTAG_COMMAND, commands::JTAG_CLOSE_AP_DBG, apsel],
                &[],
                &mut buf,
                TIMEOUT,
            )?;
            Self::check_status(&buf)
        }
    }

    /// Drives the nRESET pin.
    /// `is_asserted` tells wheter the reset should be asserted or deasserted.
    pub fn drive_nreset(&mut self, is_asserted: bool) -> Result<(), DebugProbeError> {
        let state = if is_asserted {
            commands::JTAG_DRIVE_NRST_LOW
        } else {
            commands::JTAG_DRIVE_NRST_HIGH
        };
        let mut buf = [0; 2];
        self.device.write(
            vec![commands::JTAG_COMMAND, commands::JTAG_DRIVE_NRST, state],
            &[],
            &mut buf,
            TIMEOUT,
        )?;
        Self::check_status(&buf)
    }

    /// Validates the status given.
    /// Returns an error if the status is not `Status::JtagOk`.
    /// Returns Ok(()) otherwise.
    /// This can be called on any status returned from the attached target.
    fn check_status(status: &[u8]) -> Result<(), DebugProbeError> {
        log::trace!("check_status({:?})", status);
        if let Some(status) = Status::from_u8(status[0]) {
            if status != Status::JtagOk {
                log::warn!("check_status failed: {:?}", status);
                Err(StlinkError::CommandFailed(status).into())
            } else {
                Ok(())
            }
        } else {
            Err(StlinkError::CommandFailed(Status::Unknown).into())
        }
    }
}

#[derive(Error, Debug)]
pub(crate) enum StlinkError {
    #[error("Invalid voltage values returned by probe.")]
    VoltageDivisionByZero,
    #[error("Probe is an unknown mode.")]
    UnknownMode,
    #[error("JTAG does not support multiple APs.")]
    JTagDoesNotSupportMultipleAP,
    #[error("Blank values are not allowed on DebugPort writes.")]
    BlanksNotAllowedOnDPRegister,
    #[error("Not enough bytes read.")]
    NotEnoughBytesRead,
    #[error("USB endpoint not found.")]
    EndpointNotFound,
    #[error("Command failed with status {0:?}")]
    CommandFailed(Status),
}

impl From<StlinkError> for DebugProbeError {
    fn from(e: StlinkError) -> Self {
        DebugProbeError::ProbeSpecific(Box::new(e))
    }
}
