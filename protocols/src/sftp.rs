//! SFTP client (SSH File Transfer Protocol) — Phase 2
//! Requires SSH transport layer. This is a stub.

use alloc::string::String;
use alloc::vec::Vec;

#[derive(Debug)]
pub enum SftpError {
    NotImplemented,
    AuthFailed,
    ConnectionClosed,
    SshTransportError(String),
    Protocol(String),
}

pub enum SftpAction {
    Send(Vec<u8>),
    Ok(String),
    DataComplete(Vec<u8>),
    Error(SftpError),
    NeedMore,
}

pub struct SftpClient {
    _private: (),
}

impl SftpClient {
    pub fn new(_user: &str, _pass: &str) -> Self {
        SftpClient { _private: () }
    }

    pub fn receive(&mut self, _data: &[u8]) -> SftpAction {
        SftpAction::Error(SftpError::NotImplemented)
    }

    pub fn list(&mut self, _path: &str) -> SftpAction {
        SftpAction::Error(SftpError::NotImplemented)
    }

    pub fn get(&mut self, _remote_path: &str, _store_key: &str) -> SftpAction {
        SftpAction::Error(SftpError::NotImplemented)
    }

    pub fn put(&mut self, _remote_path: &str, _data: Vec<u8>) -> SftpAction {
        SftpAction::Error(SftpError::NotImplemented)
    }

    pub fn stat(&mut self, _path: &str) -> SftpAction {
        SftpAction::Error(SftpError::NotImplemented)
    }

    pub fn close(&mut self) -> SftpAction {
        SftpAction::Error(SftpError::NotImplemented)
    }
}
