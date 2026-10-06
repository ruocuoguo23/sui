// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use std::sync::{
    OnceLock,
    atomic::{AtomicI16, Ordering},
};

use mysten_network::Multiaddr;

struct SimAddressManager {
    next_ip_offset: AtomicI16,
    next_port: AtomicI16,
}

impl SimAddressManager {
    fn new() -> Self {
        Self {
            next_ip_offset: AtomicI16::new(1),
            next_port: AtomicI16::new(9000),
        }
    }

    fn get_new_ip(&self) -> String {
        let offset = self.next_ip_offset.fetch_add(1, Ordering::SeqCst);
        assert!(offset <= 255, "simulator IP range exhausted");
        format!("10.10.0.{offset}")
    }

    fn new_udp_address_for_testing(&self, host: &str) -> Multiaddr {
        let port = self.next_port.fetch_add(1, Ordering::SeqCst) as u16;
        format!("/ip4/{host}/udp/{port}").parse().unwrap()
    }
}

fn sim_address_manager() -> &'static SimAddressManager {
    static MANAGER: OnceLock<SimAddressManager> = OnceLock::new();
    MANAGER.get_or_init(SimAddressManager::new)
}

pub fn get_new_ip() -> String {
    sim_address_manager().get_new_ip()
}

pub fn new_udp_address_for_testing(host: &str) -> Multiaddr {
    sim_address_manager().new_udp_address_for_testing(host)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::SimAddressManager;

    #[sui_macros::sui_test]
    async fn allocates_unique_ips_in_deterministic_order() {
        let manager = SimAddressManager::new();
        let mut seen = HashSet::new();
        for offset in 1..=255 {
            let ip = manager.get_new_ip();
            assert_eq!(ip, format!("10.10.0.{offset}"));
            assert!(seen.insert(ip));
        }
    }

    #[sui_macros::sui_test]
    async fn allocates_unique_udp_ports_across_hosts() {
        let manager = SimAddressManager::new();
        let first = manager.new_udp_address_for_testing("10.10.0.1");
        let second = manager.new_udp_address_for_testing("10.10.0.2");
        let third = manager.new_udp_address_for_testing("10.10.0.1");
        assert_eq!(first.to_string(), "/ip4/10.10.0.1/udp/9000");
        assert_eq!(second.to_string(), "/ip4/10.10.0.2/udp/9001");
        assert_eq!(third.to_string(), "/ip4/10.10.0.1/udp/9002");
    }

    #[sui_macros::sui_test]
    #[should_panic(expected = "simulator IP range exhausted")]
    async fn rejects_exhausted_ip_range() {
        let manager = SimAddressManager::new();
        for _ in 1..=256 {
            manager.get_new_ip();
        }
    }
}
