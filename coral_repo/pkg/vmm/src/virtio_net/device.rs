//! VirtIO-MMIO transport plumbing: virtqueue TX/RX plumbing for the virtual NIC.

use super::{
    GUEST_MAC, VirtioNetDevice, VIRTIO_NET_F_MAC, VIRTIO_NET_F_STATUS, VIRTIO_NET_HDR_LEN,
    VIRTIO_NET_S_LINK_UP,
};
use crate::virtio_mmio::{VIRTIO_DEV_NET, VIRTIO_MMIO_INT_VRING};
use anyhow::{Context, Result};
use std::collections::VecDeque;
use std::net::TcpListener;

impl VirtioNetDevice {
    pub fn new(port_forwards: &[super::PortForwardRule]) -> Result<Self> {
        let mut listeners = Vec::new();
        for rule in port_forwards {
            let l = TcpListener::bind(("127.0.0.1", rule.host_port)).with_context(|| {
                format!(
                    "bind MicroVM VirtIO-Net port forward 127.0.0.1:{} -> guest:{}",
                    rule.host_port, rule.guest_port
                )
            })?;
            l.set_nonblocking(true)?;
            listeners.push((rule.guest_port, l));
        }

        let mut cfg = Vec::with_capacity(8);
        cfg.extend_from_slice(&GUEST_MAC);
        cfg.extend_from_slice(&VIRTIO_NET_S_LINK_UP.to_le_bytes());

        Ok(Self {
            transport: crate::virtio_mmio::VirtioMmioTransport::new(
                VIRTIO_DEV_NET,
                VIRTIO_NET_F_MAC | VIRTIO_NET_F_STATUS,
                0,
                2,
                cfg,
            ),
            guest_mac: GUEST_MAC,
            rx_pending: VecDeque::new(),
            tcp_flows: std::collections::HashMap::new(),
            udp_flows: std::collections::HashMap::new(),
            listeners,
            next_ephemeral_port: 49152,
            ip_ident: 1,
            dns_upstream: super::checksum::detect_host_ipv4_dns(),
            tx_packets: 0,
            rx_packets: 0,
        })
    }

    pub fn read_mmio(&self, offset: u64, data: &mut [u8]) {
        self.transport.read_mmio(offset, data);
    }

    /// Processes an MMIO write and handles TX/RX virtqueue notifications.
    pub fn write_mmio(&mut self, offset: u64, data: &[u8], mem: &mut [u8]) -> bool {
        let Some(qidx) = self.transport.write_mmio(offset, data) else {
            return false;
        };
        match qidx {
            1 => {
                self.drain_tx(mem);
                self.flush_rx(mem)
            }
            0 => self.flush_rx(mem),
            _ => false,
        }
    }

    /// Polls active TCP/UDP sockets + port-forward listeners and delivers RX frames.
    pub fn poll(&mut self, mem: &mut [u8]) -> bool {
        self.poll_listeners();
        self.poll_tcp_flows();
        self.poll_udp_flows();
        self.flush_rx(mem)
    }

    fn drain_tx(&mut self, mem: &mut [u8]) {
        while let Some(chain) = self.transport.queues[1].pop_avail_chain(mem) {
            let mut packet = Vec::with_capacity(1536);
            for seg in &chain.segments {
                if !seg.write_only {
                    let s = seg.addr as usize;
                    packet.extend_from_slice(&mem[s..s + seg.len as usize]);
                }
            }
            let _ = self.transport.queues[1].push_used(mem, chain.head, 0);
            self.transport.interrupt_status |= VIRTIO_MMIO_INT_VRING;

            if packet.len() > VIRTIO_NET_HDR_LEN {
                self.tx_packets = self.tx_packets.saturating_add(1);
                let frame = &packet[VIRTIO_NET_HDR_LEN..];
                self.handle_guest_frame(frame);
            }
        }
    }

    fn flush_rx(&mut self, mem: &mut [u8]) -> bool {
        let mut delivered = false;
        while let Some(frame) = self.rx_pending.front() {
            let need_len = VIRTIO_NET_HDR_LEN + frame.len();
            let Some(chain) = self.transport.queues[0].pop_avail_chain(mem) else {
                break;
            };
            let frame = self.rx_pending.pop_front().unwrap();
            let mut full = Vec::with_capacity(need_len);
            full.extend_from_slice(&[0u8; VIRTIO_NET_HDR_LEN]);
            full.extend_from_slice(&frame);

            let mut offset = 0usize;
            for seg in &chain.segments {
                if !seg.write_only || offset >= full.len() {
                    continue;
                }
                let take = (seg.len as usize).min(full.len() - offset);
                let dst = &mut mem[seg.addr as usize..seg.addr as usize + take];
                dst.copy_from_slice(&full[offset..offset + take]);
                offset += take;
            }
            let _ = self
                .transport
                .queues[0]
                .push_used(mem, chain.head, offset as u32);
            self.transport.interrupt_status |= VIRTIO_MMIO_INT_VRING;
            self.rx_packets = self.rx_packets.saturating_add(1);
            delivered = true;
        }
        delivered
    }

    #[cfg(test)]
    pub(crate) fn pop_rx_frame_for_test(&mut self) -> Option<Vec<u8>> {
        self.rx_pending.pop_front()
    }
}
