#!/usr/bin/env python3
# Copyright (c) Mysten Labs, Inc.
# SPDX-License-Identifier: Apache-2.0

import errno
import socket
import unittest

from cluster import reserve_ports


class PortReservationTests(unittest.TestCase):
    def test_active_listener_cannot_be_reserved_again(self):
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
            listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            listener.bind(("127.0.0.1", 0))
            listener.listen(1)
            with self.assertRaises(OSError) as error:
                reserve_ports([listener.getsockname()[1]])
            self.assertEqual(error.exception.errno, errno.EADDRINUSE)

    def test_recently_closed_server_port_can_be_reserved(self):
        listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        listener.bind(("127.0.0.1", 0))
        listener.listen(1)
        port = listener.getsockname()[1]
        client = socket.create_connection(("127.0.0.1", port))
        accepted, _ = listener.accept()
        accepted.close()
        self.assertEqual(client.recv(1), b"")
        client.close()
        listener.close()
        reservations = reserve_ports([port])
        try:
            self.assertEqual(reservations[port].getsockname()[1], port)
        finally:
            for reservation in reservations.values():
                reservation.close()


if __name__ == "__main__":
    unittest.main()
