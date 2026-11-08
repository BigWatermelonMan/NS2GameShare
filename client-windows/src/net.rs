//! User-space Ethernet stack; no TAP driver or administrator network configuration.
use anyhow::{Result, ensure};
use smoltcp::{
    iface::{Config, Interface, SocketHandle, SocketSet},
    phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken},
    socket::{tcp, udp},
    time::Instant,
    wire::{EthernetAddress, IpAddress, IpCidr},
};
use std::collections::VecDeque;
const QUEUE: usize = 256;
#[derive(Default)]
pub struct EthernetDevice {
    pub rx: VecDeque<Vec<u8>>,
    pub tx: VecDeque<Vec<u8>>,
}
pub struct Rx(Vec<u8>);
pub struct Tx<'a>(&'a mut VecDeque<Vec<u8>>);
impl RxToken for Rx {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.0)
    }
}
impl TxToken for Tx<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut b = vec![0; len];
        let r = f(&mut b);
        self.0.push_back(b);
        r
    }
}
impl Device for EthernetDevice {
    type RxToken<'a> = Rx;
    type TxToken<'a> = Tx<'a>;
    fn receive(&mut self, _: Instant) -> Option<(Rx, Tx<'_>)> {
        if self.tx.len() >= QUEUE {
            return None;
        }
        let p = self.rx.pop_front()?;
        Some((Rx(p), Tx(&mut self.tx)))
    }
    fn transmit(&mut self, _: Instant) -> Option<Tx<'_>> {
        if self.tx.len() < QUEUE {
            Some(Tx(&mut self.tx))
        } else {
            None
        }
    }
    fn capabilities(&self) -> DeviceCapabilities {
        let mut c = DeviceCapabilities::default();
        c.medium = Medium::Ethernet;
        c.max_transmission_unit = 1514;
        c.max_burst_size = Some(8);
        c
    }
}
pub struct Network {
    pub device: EthernetDevice,
    iface: Interface,
    sockets: SocketSet<'static>,
    tcp: SocketHandle,
    pub received: u64,
    pub connected: bool,
}
impl Network {
    pub fn new(mac: [u8; 6], ip: [u8; 4], host: [u8; 4], port: u16, now: Instant) -> Result<Self> {
        let mut device = EthernetDevice::default();
        let mut config = Config::new(EthernetAddress(mac).into());
        let mut seed = [0; 8];
        getrandom::fill(&mut seed).map_err(|_| anyhow::anyhow!("random source failed"))?;
        config.random_seed = u64::from_le_bytes(seed);
        let mut iface = Interface::new(config, &mut device, now);
        iface.update_ip_addrs(|a| {
            a.push(IpCidr::new(IpAddress::v4(ip[0], ip[1], ip[2], ip[3]), 16))
                .unwrap()
        });
        let mut sockets = SocketSet::new(vec![]);
        let mut socket = tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0; 256 * 1024]),
            tcp::SocketBuffer::new(vec![0; 64 * 1024]),
        );
        socket.set_nagle_enabled(false);
        socket.set_timeout(Some(smoltcp::time::Duration::from_secs(10)));
        let tcp = sockets.add(socket);
        sockets
            .get_mut::<tcp::Socket>(tcp)
            .connect(
                iface.context(),
                (IpAddress::v4(host[0], host[1], host[2], host[3]), port),
                49152 | (u16::from_le_bytes([seed[0], seed[1]]) & 0x3fff),
            )
            .map_err(|e| anyhow::anyhow!("TCP connect: {e:?}"))?;
        Ok(Self {
            device,
            iface,
            sockets,
            tcp,
            received: 0,
            connected: false,
        })
    }
    pub fn receive(&mut self, p: &[u8]) -> Result<()> {
        ensure!(self.device.rx.len() < QUEUE, "network RX queue full");
        self.device.rx.push_back(p.to_vec());
        Ok(())
    }
    pub fn poll(&mut self, now: Instant) -> Result<Vec<u8>> {
        self.iface.poll(now, &mut self.device, &mut self.sockets);
        let s = self.sockets.get_mut::<tcp::Socket>(self.tcp);
        self.connected = s.state() == tcp::State::Established;
        let mut out = Vec::new();
        while s.can_recv() {
            s.recv(|b| {
                out.extend_from_slice(b);
                (b.len(), ())
            })
            .map_err(|e| anyhow::anyhow!("TCP receive: {e:?}"))?;
        }
        self.received += out.len() as u64;
        Ok(out)
    }
    pub fn send(&mut self, b: &[u8]) -> Result<usize> {
        self.sockets
            .get_mut::<tcp::Socket>(self.tcp)
            .send_slice(b)
            .map_err(|e| anyhow::anyhow!("TCP send: {e:?}"))
    }
    pub fn state(&self) -> tcp::State {
        self.sockets.get::<tcp::Socket>(self.tcp).state()
    }
    pub fn open_udp(&mut self, port: u16) -> Result<SocketHandle> {
        let mut socket = udp::Socket::new(
            udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 256], vec![0; 512 * 1024]),
            udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 32], vec![0; 64 * 1024]),
        );
        socket
            .bind(port)
            .map_err(|e| anyhow::anyhow!("UDP bind: {e:?}"))?;
        Ok(self.sockets.add(socket))
    }
    pub fn receive_udp(&mut self, handle: SocketHandle) -> Result<Vec<Vec<u8>>> {
        let s = self.sockets.get_mut::<udp::Socket>(handle);
        let mut out = Vec::new();
        while s.can_recv() {
            let (data, _meta) = s
                .recv()
                .map_err(|e| anyhow::anyhow!("UDP receive: {e:?}"))?;
            out.push(data.to_vec());
        }
        Ok(out)
    }
    pub fn send_udp(
        &mut self,
        handle: SocketHandle,
        data: &[u8],
        host: [u8; 4],
        port: u16,
    ) -> Result<bool> {
        let socket = self.sockets.get_mut::<udp::Socket>(handle);
        if !socket.can_send() {
            return Ok(false);
        }
        socket
            .send_slice(
                data,
                (IpAddress::v4(host[0], host[1], host[2], host[3]), port),
            )
            .map_err(|e| anyhow::anyhow!("UDP send: {e:?}"))?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ethernet_adapter_completes_tcp_and_delivers_data() {
        let mac = [2, 0, 0, 0, 0, 1];
        let mut server_device = EthernetDevice::default();
        let mut server = Interface::new(
            Config::new(EthernetAddress(mac).into()),
            &mut server_device,
            Instant::from_millis(0),
        );
        server.update_ip_addrs(|a| {
            a.push(IpCidr::new(IpAddress::v4(169, 254, 1, 1), 16))
                .unwrap()
        });
        let mut sockets = SocketSet::new(vec![]);
        let mut socket = tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0; 4096]),
            tcp::SocketBuffer::new(vec![0; 4096]),
        );
        socket.listen(12345).unwrap();
        let handle = sockets.add(socket);
        let mut client = Network::new(
            [2, 0, 0, 0, 0, 2],
            [169, 254, 1, 2],
            [169, 254, 1, 1],
            12345,
            Instant::from_millis(0),
        )
        .unwrap();
        let mut sent = false;
        let mut received = Vec::new();
        for tick in 0..1000 {
            let now = Instant::from_millis(tick);
            received.extend(client.poll(now).unwrap());
            while let Some(p) = client.device.tx.pop_front() {
                server_device.rx.push_back(p);
            }
            server.poll(now, &mut server_device, &mut sockets);
            let socket = sockets.get_mut::<tcp::Socket>(handle);
            if socket.can_send() && !sent {
                socket.send_slice(b"GameShare control transport").unwrap();
                sent = true;
            }
            while let Some(p) = server_device.tx.pop_front() {
                client.receive(&p).unwrap();
            }
            if received == b"GameShare control transport" {
                break;
            }
        }
        assert!(client.connected);
        assert_eq!(received, b"GameShare control transport");
    }
}
