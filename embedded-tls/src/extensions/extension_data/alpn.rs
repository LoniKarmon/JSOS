use heapless::Vec;
use crate::{
    buffer::CryptoBuffer,
    parse_buffer::{ParseBuffer, ParseError},
    TlsError,
};

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct ProtocolName<'a> {
    pub name: &'a [u8],
}

impl<'a> ProtocolName<'a> {
    pub fn parse(buf: &mut ParseBuffer<'a>) -> Result<Self, ParseError> {
        let len = buf.read_u8()? as usize;
        let name = buf.slice(len)?.as_slice();
        Ok(Self { name })
    }

    pub fn encode(&self, buf: &mut CryptoBuffer) -> Result<(), TlsError> {
        buf.push(self.name.len() as u8).map_err(|_| TlsError::EncodeError)?;
        buf.extend_from_slice(self.name).map_err(|_| TlsError::EncodeError)
    }
}

/// ALPN protocol name list extension (RFC 7301).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct AlpnProtocolList<'a, const N: usize> {
    pub protocols: Vec<ProtocolName<'a>, N>,
}

impl<'a, const N: usize> AlpnProtocolList<'a, N> {
    pub fn parse(buf: &mut ParseBuffer<'a>) -> Result<Self, ParseError> {
        let list_len = buf.read_u16()? as usize;
        let protocols = buf.read_list::<_, N>(list_len, ProtocolName::parse)?;
        Ok(Self { protocols })
    }

    pub fn encode(&self, buf: &mut CryptoBuffer) -> Result<(), TlsError> {
        buf.with_u16_length(|buf| {
            for protocol in self.protocols.iter() {
                protocol.encode(buf)?;
            }
            Ok(())
        })
    }
}
