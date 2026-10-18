#!/usr/bin/env python3
"""Lets the program start on systems with an older glibc (Linux's C library).

Rust's standard library uses a few new glibc functions (pidfd_spawnp, pidfd_getpid)
only when the system has them, so they're linked "weakly". But the program also
records that it wants glibc 2.39 for them, and an older system refuses to start it.
This marks that wish as optional (VER_FLG_WEAK), as the linker would have if it knew.

    python3 weak-glibc.py path/to/program
"""
import struct
import sys

VERSIONS = {b"GLIBC_2.36", b"GLIBC_2.37", b"GLIBC_2.38", b"GLIBC_2.39", b"GLIBC_2.40", b"GLIBC_2.41"}
VER_FLG_WEAK = 0x2

path = sys.argv[1]
data = bytearray(open(path, "rb").read())
assert data[:4] == b"\x7fELF" and data[4] == 2, "a 64-bit Linux program"
shoff, = struct.unpack_from("<Q", data, 0x28)
shentsize, shnum, shstrndx = struct.unpack_from("<HHH", data, 0x3A)

def section(i):
    name, kind, flags, addr, offset, size, link = struct.unpack_from("<IIQQQQI", data, shoff + i * shentsize)
    return name, kind, offset, size, link

names_offset = section(shstrndx)[2]
def text(at):
    return bytes(data[at:data.index(0, at)])

for i in range(shnum):
    name, kind, offset, size, link = section(i)
    if text(names_offset + name) != b".gnu.version_r":
        continue
    strings = section(link)[2]
    entry = offset
    while True:  # Elf64_Verneed: version, count, file, aux, next
        _, count, _, aux, next_entry = struct.unpack_from("<HHIII", data, entry)
        at = entry + aux
        for _ in range(count):  # Elf64_Vernaux: hash, flags, other, name, next
            _, flags, _, name_at, next_aux = struct.unpack_from("<IHHII", data, at)
            version = text(strings + name_at)
            if version in VERSIONS and not flags & VER_FLG_WEAK:
                struct.pack_into("<H", data, at + 4, flags | VER_FLG_WEAK)
                print(f"{version.decode()} is optional now")
            at += next_aux
        if not next_entry:
            break
        entry += next_entry

open(path, "wb").write(data)
