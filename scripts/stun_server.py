import sys, socket, struct, ipaddress
MAGIC = 0x2112A442
def run(host='0.0.0.0', port=3478):
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.bind((host, port))
    print(f'[STUN Server] Listening on {host}:{port}...', flush=True)
    while True:
        try:
            data, addr = s.recvfrom(2048)
            if len(data) < 20: continue
            msg_type, msg_len, cookie = struct.unpack('!HHI', data[:8])
            if msg_type != 1 or cookie != MAGIC: continue
            tx_id = data[8:20]
            ip_obj = ipaddress.ip_address(addr[0])
            if ip_obj.version == 4:
                fam = 1
                p_xor = addr[1] ^ (MAGIC >> 16)
                ip_xor = bytes(b ^ c for b, c in zip(ip_obj.packed, struct.pack('!I', MAGIC)))
                val = struct.pack('!BBH', 0, fam, p_xor) + ip_xor
                attr = struct.pack('!HH', 0x0020, len(val)) + val
            else: continue
            resp = struct.pack('!HHI', 0x0101, len(attr), MAGIC) + tx_id + attr
            s.sendto(resp, addr)
            print(f'[STUN Server] Handled request from {addr[0]}:{addr[1]} -> Mapped: {addr[0]}:{addr[1]}', flush=True)
        except KeyboardInterrupt: break
        except Exception as e: print(f'[STUN Server] Error: {e}', flush=True)
if __name__ == '__main__':
    p = int(sys.argv[1]) if len(sys.argv) > 1 else 3478
    run(port=p)
