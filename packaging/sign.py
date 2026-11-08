"""Signs release files for the in-app updater.

    python packaging/sign.py <file>...

Writes <file>.sig next to each file: the raw 64-byte ECDSA P-256 signature
(r || s, big-endian) over the file's SHA-256. The private key is read (PEM)
from the UPDATE_SIGNING_KEY environment variable; 3zT Mirroring checks the
signature with the public key built into it (rust/crates/mirror-app/src/updater.rs).
"""

import os
import sys

from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives.asymmetric.utils import decode_dss_signature, encode_dss_signature

EXPECTED_PUBLIC = (
    "474b36c6016fa864f370b95c6a078444c78a900fc9462f679591c199b88104ba"
    "0387322126f3ee64c6104d5df01f4cdbe757a2ff9a59ef87dda4565ed8295bfe"
)


def main() -> int:
    pem = os.environ.get("UPDATE_SIGNING_KEY", "").strip()
    if not pem:
        print("UPDATE_SIGNING_KEY is not set", file=sys.stderr)
        return 1
    key = serialization.load_pem_private_key(pem.encode(), password=None)
    if not isinstance(key, ec.EllipticCurvePrivateKey) or key.curve.name != "secp256r1":
        print("UPDATE_SIGNING_KEY is not a P-256 key", file=sys.stderr)
        return 1
    pub = key.public_key().public_numbers()
    if pub.x.to_bytes(32, "big").hex() + pub.y.to_bytes(32, "big").hex() != EXPECTED_PUBLIC:
        print("UPDATE_SIGNING_KEY does not match the public key in the app", file=sys.stderr)
        return 1
    for path in sys.argv[1:]:
        with open(path, "rb") as fh:
            data = fh.read()
        r, s = decode_dss_signature(key.sign(data, ec.ECDSA(hashes.SHA256())))
        sig = r.to_bytes(32, "big") + s.to_bytes(32, "big")
        # Check the written form round-trips before it ships.
        key.public_key().verify(encode_dss_signature(r, s), data, ec.ECDSA(hashes.SHA256()))
        with open(path + ".sig", "wb") as fh:
            fh.write(sig)
        print(f"signed {os.path.basename(path)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
