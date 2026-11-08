#!/usr/bin/env python3
"""Joins the accessibility bus and then answers nothing, like an app that hangs just after connecting."""
import os
import socket
import struct
import subprocess
import time

reply = subprocess.check_output(
    ["busctl", "--user", "call", "org.a11y.Bus", "/org/a11y/bus", "org.a11y.Bus", "GetAddress"], text=True
)
address = reply.split('"')[1]
path = address.split("path=")[1].split(",")[0]

bus = socket.socket(socket.AF_UNIX)
bus.connect(path)
bus.sendall(b"\0AUTH EXTERNAL " + str(os.getuid()).encode().hex().encode() + b"\r\n")
bus.recv(256)
bus.sendall(b"BEGIN\r\n")

# The bus's Hello call, which every client makes first: without a D-Bus library, as bytes.
fields = b""
for code, signature, value in [
    (1, b"o", b"/org/freedesktop/DBus"),
    (2, b"s", b"org.freedesktop.DBus"),
    (3, b"s", b"Hello"),
    (6, b"s", b"org.freedesktop.DBus"),
]:
    fields += b"\0" * (-len(fields) % 8)
    fields += bytes([code, 1]) + signature + b"\0"
    fields += b"\0" * (-len(fields) % 4)
    fields += struct.pack("<I", len(value)) + value + b"\0"
hello = b"l\x01\0\x01" + struct.pack("<III", 0, 1, len(fields)) + fields
bus.sendall(hello + b"\0" * (-len(hello) % 8))

time.sleep(300)
