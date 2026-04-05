use alloc::vec::Vec;
use alloc::vec;
use smoltcp::phy::{Device, DeviceCapabilities, RxToken, TxToken, Medium};
use smoltcp::time::Instant;
use super::rtl8139::Rtl8139;

pub trait NicDriver {
    fn mac_address(&self) -> [u8; 6];
    fn receive_packet(&mut self) -> Option<Vec<u8>>;
    fn transmit_packet(&mut self, data: &[u8]);
    fn capabilities(&self) -> DeviceCapabilities;
}

pub enum Nic {
    Rtl8139(Rtl8139),
}

impl NicDriver for Nic {
    fn mac_address(&self) -> [u8; 6] {
        match self { Nic::Rtl8139(n) => n.mac_address() }
    }
    fn receive_packet(&mut self) -> Option<Vec<u8>> {
        match self { Nic::Rtl8139(n) => n.receive_packet() }
    }
    fn transmit_packet(&mut self, data: &[u8]) {
        match self { Nic::Rtl8139(n) => n.transmit_packet(data) }
    }
    fn capabilities(&self) -> DeviceCapabilities {
        match self { Nic::Rtl8139(n) => NicDriver::capabilities(n) }
    }
}

pub struct NicRxToken(Vec<u8>);

impl RxToken for NicRxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut packet = self.0;
        f(&mut packet)
    }
}

pub struct NicTxToken<'a>(&'a mut Nic);

impl<'a> TxToken for NicTxToken<'a> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buffer = vec![0; len];
        let result = f(&mut buffer);
        self.0.transmit_packet(&buffer);
        result
    }
}

impl Device for Nic {
    type RxToken<'a> = NicRxToken;
    type TxToken<'a> = NicTxToken<'a>;

    fn receive<'a>(&'a mut self, _timestamp: Instant) -> Option<(Self::RxToken<'a>, Self::TxToken<'a>)> {
        match self.receive_packet() {
            Some(packet) => Some((NicRxToken(packet), NicTxToken(self))),
            None => None,
        }
    }

    fn transmit<'a>(&'a mut self, _timestamp: Instant) -> Option<Self::TxToken<'a>> {
        Some(NicTxToken(self))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        NicDriver::capabilities(self)
    }
}
