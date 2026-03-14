#!/usr/bin/env python3
import jwt
import time
import datetime
import argparse
import sys

def parse_args():
    parser = argparse.ArgumentParser(description="JWT Token Generator")
    parser.add_argument("--secret", required=True, help="Secret key for signing the JWT")
    parser.add_argument("--origin", required=True, help="Origin claim value")
    parser.add_argument("--expiry-years", type=int, default=5, help="Expiry in years (default: 5)")
    parser.add_argument("--algorithm", default="HS256", help="JWT signing algorithm (default: HS256)")
    parser.add_argument("--show-payload", action="store_true", help="Print the JWT payload before encoding")
    return parser.parse_args()

def main():
    args = parse_args()
    now_unix = int(time.time())
    expiry_unix = now_unix + args.expiry_years * 365 * 24 * 60 * 60
    gendate = datetime.datetime.fromtimestamp(now_unix).isoformat() + "Z"
    payload = {
        "origin": args.origin,
        "gendate": gendate,
        "exp": expiry_unix,
    }
    if args.show_payload:
        print("Payload:", payload, file=sys.stderr)
    token = jwt.encode(payload, args.secret, algorithm=args.algorithm)
    print(token)

if __name__ == "__main__":
    main()