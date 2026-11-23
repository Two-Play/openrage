#!/usr/bin/env python3
"""RPF9 reference-build path hash primitives. No game keys or executable needed.
Mount/extension processing is separate; see README.txt.
"""
import struct
import argparse

MASK=(1<<64)-1
C1=0x87c37b91114253d5
C2=0x4cf5ad432745937f
def rol(x,n):
    return ((x<<n)|(x>>(64-n)))&MASK
def fmix(x):
    x=((x^(x>>33))*0xff51afd7ed558ccd)&MASK
    x=((x^(x>>33))*0xc4ceb9fe1a85ec53)&MASK
    return x^(x>>33)
def native_hash(data,table):
    data=data.split(b'\0',1)[0].translate(table)
    h1=h2=0
    end=len(data)&~15
    for i in range(0,end,16):
        k1,k2=struct.unpack_from('<QQ',data,i)
        h1 ^= (rol((k1*C1)&MASK,31)*C2)&MASK
        h1=((rol(h1,27)+h2)*5+0x52dce729)&MASK
        h2 ^= (rol((k2*C2)&MASK,33)*C1)&MASK
        h2=((rol(h2,31)+h1)*5+0x38495ab5)&MASK
    tail=data[end:]
    if len(tail)>8:
        h2^=(rol((int.from_bytes(tail[8:],'little')*C2)&MASK,33)*C1)&MASK
    if tail:
        h1^=(rol((int.from_bytes(tail[:8],'little')*C1)&MASK,31)*C2)&MASK
    h1^=len(data)&255
    h2^=len(data)&255
    h1=(h1+h2)&MASK
    h2=(h2+h1)&MASK
    return (fmix(h1)+fmix(h2))&MASK

NORMALIZATION = bytes(i + 32 if 65 <= i <= 90 else 47 if i == 92 else i for i in range(256))

def hash64(path):
    data = path.encode('utf-8') if isinstance(path, str) else path
    return native_hash(data, NORMALIZATION)

def hash32(path, seed=0):
    data = path.encode('utf-8') if isinstance(path, str) else path
    data = data.split(b'\0', 1)[0]
    if data.startswith(b'"'):
        data = data[1:].split(b'"', 1)[0]
    h = seed & 0xffffffff
    for b in data.translate(NORMALIZATION):
        h = (h + b) & 0xffffffff
        h = (h + (h << 10)) & 0xffffffff
        h ^= h >> 6
    h = (h + (h << 3)) & 0xffffffff
    h ^= h >> 11
    return (h + (h << 15)) & 0xffffffff

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('paths', nargs='*')
    parser.add_argument('--self-test', action='store_true')
    args = parser.parse_args()
    if args.self_test:
        assert hash64('') == 0
        assert hash64('DATA\\graphics/graphics_definitions.meta') == 0x5c33030f6402b9dd
        assert hash64('data/camera/cam_core') == 0x43ba57fb34f73a27
        assert hash64('a'*256) == 0x42bffdb35fe459a8
        assert hash32('ADDER') == 0xb779a091
        assert hash32('"adder"unused') == hash32('adder')
        print('self-test: OK')
    for path in args.paths:
        print(f'{hash64(path):016x}\t{hash32(path):08x}\t{path}')
