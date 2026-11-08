#!/usr/bin/env python3
"""Runs JavaScript in the test's Chromium page through the DevTools protocol, with nothing but
Python's standard library (the test machine has no websocket package).

  cdp.py PORT URL_PART 'expression'     prints the expression's value as JSON

The smoke tests use it to focus and select a field and to read what the page holds afterwards. Keys
never go through it: the user's keys come from the virtual machine's keyboard, so they take the
same path a person's do.
"""
import base64
import json
import os
import socket
import struct
import sys
import urllib.request


def page(port, url_part):
    with urllib.request.urlopen(f"http://127.0.0.1:{port}/json/list", timeout=5) as answer:
        targets = json.load(answer)
    for target in targets:
        if target.get("type") == "page" and url_part in target.get("url", ""):
            return target["webSocketDebuggerUrl"]
    raise SystemExit(f"no page with {url_part!r} in its address")


def connect(ws_url):
    rest = ws_url.split("://", 1)[1]
    hostport, path = rest.split("/", 1)
    host, port = hostport.split(":")
    sock = socket.create_connection((host, int(port)), timeout=10)
    key = base64.b64encode(os.urandom(16)).decode()
    sock.sendall(
        f"GET /{path} HTTP/1.1\r\nHost: {hostport}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
        f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n".encode()
    )
    head = b""
    while b"\r\n\r\n" not in head:
        chunk = sock.recv(4096)
        if not chunk:
            raise SystemExit("DevTools closed the connection")
        head += chunk
    if b" 101 " not in head.split(b"\r\n", 1)[0]:
        raise SystemExit(f"DevTools refused the connection: {head[:80]!r}")
    return sock


def send(sock, text):
    data = text.encode()
    mask = os.urandom(4)
    size = len(data)
    if size < 126:
        header = struct.pack("!BB", 0x81, 0x80 | size)
    elif size < 65536:
        header = struct.pack("!BBH", 0x81, 0x80 | 126, size)
    else:
        header = struct.pack("!BBQ", 0x81, 0x80 | 127, size)
    sock.sendall(header + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(data)))


def exactly(sock, n):
    data = b""
    while len(data) < n:
        chunk = sock.recv(n - len(data))
        if not chunk:
            raise SystemExit("DevTools closed the connection")
        data += chunk
    return data


def receive(sock):
    message = b""
    while True:
        first, second = exactly(sock, 2)
        size = second & 0x7F
        if size == 126:
            size = struct.unpack("!H", exactly(sock, 2))[0]
        elif size == 127:
            size = struct.unpack("!Q", exactly(sock, 8))[0]
        message += exactly(sock, size)
        if first & 0x80:
            return message.decode()


def evaluate(port, url_part, expression):
    sock = connect(page(port, url_part))
    params = {"expression": expression, "returnByValue": True, "awaitPromise": True, "userGesture": True}
    send(sock, json.dumps({"id": 1, "method": "Runtime.evaluate", "params": params}))
    while True:
        answer = json.loads(receive(sock))
        if answer.get("id") == 1:
            break
    sock.close()
    result = answer.get("result", {})
    if "exceptionDetails" in result:
        raise SystemExit(f"the page threw: {result['exceptionDetails'].get('text')}")
    return result.get("result", {}).get("value")


if __name__ == "__main__":
    if len(sys.argv) != 4:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    print(json.dumps(evaluate(int(sys.argv[1]), sys.argv[2], sys.argv[3]), ensure_ascii=False))
