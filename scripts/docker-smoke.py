#!/usr/bin/env python3
"""Exercise the actual scratch image, network, healthcheck and SIGTERM. Stdlib only."""
import http.client
import json
import socket
import socketserver
import struct
import subprocess
import sys
import threading
import time
import uuid


def docker(*args):
    return subprocess.check_output(["docker", *args], text=True).strip()


def exact(stream, size):
    data = bytearray()
    while len(data) < size:
        part = stream.recv(size - len(data))
        if not part:
            raise EOFError()
        data.extend(part)
    return bytes(data)


class Device(socketserver.BaseRequestHandler):
    def handle(self):
        self.request.settimeout(10)
        try:
            while True:
                header = exact(self.request, 6)
                body = exact(self.request, int.from_bytes(header[4:], "big"))
                reply = body[:2] + bytes([2, 0, 42])
                self.request.sendall(header[:4] + len(reply).to_bytes(2, "big") + reply)
        except (EOFError, OSError):
            pass


class Server(socketserver.ThreadingTCPServer):
    daemon_threads = True


def main():
    image = sys.argv[1] if len(sys.argv) > 1 else "modbus-relay:local"
    name = "modbus-relay-smoke-" + uuid.uuid4().hex[:10]
    server = Server(("0.0.0.0", 0), Device)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        docker("run", "-d", "--name", name, "--read-only", "--cap-drop=ALL",
               "--security-opt=no-new-privileges:true", "--memory=32m", "--pids-limit=32",
               "--add-host=host.docker.internal:host-gateway", "-p", "127.0.0.1::5020",
               "-p", "127.0.0.1::8080", image, "--modbus",
               f"host.docker.internal:{server.server_address[1]}", "-b", "0.0.0.0:5020")
        info = json.loads(docker("inspect", name))[0]
        ports = info["NetworkSettings"]["Ports"]
        modbus_port = int(ports["5020/tcp"][0]["HostPort"])
        http_port = int(ports["8080/tcp"][0]["HostPort"])
        assert info["Config"]["User"] == "65532:65532"

        def get(path):
            conn = http.client.HTTPConnection("127.0.0.1", http_port, timeout=3)
            try:
                conn.request("GET", path)
                response = conn.getresponse()
                return response.status, response.read().decode()
            finally:
                conn.close()

        for _ in range(100):
            try:
                if get("/healthz")[0] == 200:
                    break
            except OSError:
                pass
            time.sleep(0.1)
        else:
            raise AssertionError("container did not become live")
        assert get("/health")[0] == 503, "unprobed device must not be healthy"
        with socket.create_connection(("127.0.0.1", modbus_port), timeout=3) as stream:
            for tid in range(100):
                stream.sendall(struct.pack(">HHHBBHH", tid, 0, 6, 1, 3, 0, 1))
                assert exact(stream, 11) == struct.pack(">HHHBBB H", tid, 0, 5, 1, 3, 2, 42)
        assert get("/health")[0] == 200
        status, metrics = get("/metrics")
        assert status == 200
        assert 'modbus_relay_responses_total{device="device_1"} 100\n' in metrics
        docker("exec", name, "/modbus-relay", "--healthcheck", "127.0.0.1:8080")
        print(docker("stats", "--no-stream", "--format", "{{.MemUsage}}", name))
        docker("stop", "--time=8", name)
        state = json.loads(docker("inspect", name))[0]["State"]
        assert state["ExitCode"] == 0, state
        assert not state["OOMKilled"]
        print("Docker smoke passed: forwarding, non-root/read-only, health, metrics, healthcheck, SIGTERM")
    except Exception:
        subprocess.run(["docker", "logs", name], check=False)
        raise
    finally:
        subprocess.run(["docker", "rm", "-f", name], stdout=subprocess.DEVNULL, check=False)
        server.shutdown()
        server.server_close()


if __name__ == "__main__":
    main()
