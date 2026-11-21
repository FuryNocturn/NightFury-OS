// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tare Fernandez Salas (FuryNocturn) — ver NOTICE
//! v0.12.1: sockets UDP, puertos TCP a la escucha y lista de conexiones. Reglas en `mod.rs`.
//!
//! CONTRATO (paso 1): solo esqueleto; el paso 2 lo rellena.
use super::*;

impl Net {
    pub(super) fn udp_open(&mut self, _port: u16) -> Result<UdpId, NetError> {
        Err(NetError::Busy)
    }
    pub(super) fn udp_write(&mut self, _s: UdpId, _dst: Ipv4, _port: u16, _data: &[u8]) -> Result<(), NetError> {
        Err(NetError::Gone)
    }
    pub(super) fn udp_read(&mut self, _s: UdpId) -> Result<Option<Datagram>, NetError> {
        Err(NetError::Gone)
    }
    pub(super) fn udp_shut(&mut self, _s: UdpId) {}
    pub(super) fn listen_open(&mut self, _port: u16) -> Result<ListenId, NetError> {
        Err(NetError::Busy)
    }
    pub(super) fn listen_accept(&mut self, _l: ListenId) -> Result<Option<(SockId, Ipv4, u16)>, NetError> {
        Err(NetError::Gone)
    }
    pub(super) fn listen_shut(&mut self, _l: ListenId) {}
    pub(super) fn conn_list(&self) -> Vec<ConnInfo> {
        Vec::new()
    }
}
