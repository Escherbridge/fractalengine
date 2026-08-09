import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createPublicKey, verify } from "node:crypto";
import test from "node:test";

const fixture = JSON.parse(
  readFileSync(new URL("./operation-envelope-v1.json", import.meta.url)),
);

const INITIALIZATION_VECTOR = [
  0x6a09e667,
  0xbb67ae85,
  0x3c6ef372,
  0xa54ff53a,
  0x510e527f,
  0x9b05688c,
  0x1f83d9ab,
  0x5be0cd19,
];
const MESSAGE_PERMUTATION = [2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8];
const CHUNK_START = 1;
const CHUNK_END = 2;
const ROOT = 8;
const U64_MAX = 0xffff_ffff_ffff_ffffn;

function add32(...values) {
  return values.reduce((sum, value) => (sum + value) >>> 0, 0);
}

function rotateRight32(value, count) {
  return ((value >>> count) | (value << (32 - count))) >>> 0;
}

function mix(state, a, b, c, d, left, right) {
  state[a] = add32(state[a], state[b], left);
  state[d] = rotateRight32(state[d] ^ state[a], 16);
  state[c] = add32(state[c], state[d]);
  state[b] = rotateRight32(state[b] ^ state[c], 12);
  state[a] = add32(state[a], state[b], right);
  state[d] = rotateRight32(state[d] ^ state[a], 8);
  state[c] = add32(state[c], state[d]);
  state[b] = rotateRight32(state[b] ^ state[c], 7);
}

function round(state, message) {
  mix(state, 0, 4, 8, 12, message[0], message[1]);
  mix(state, 1, 5, 9, 13, message[2], message[3]);
  mix(state, 2, 6, 10, 14, message[4], message[5]);
  mix(state, 3, 7, 11, 15, message[6], message[7]);
  mix(state, 0, 5, 10, 15, message[8], message[9]);
  mix(state, 1, 6, 11, 12, message[10], message[11]);
  mix(state, 2, 7, 8, 13, message[12], message[13]);
  mix(state, 3, 4, 9, 14, message[14], message[15]);
}

function compress(chainingValue, block, blockLength, flags) {
  const state = [
    ...chainingValue,
    ...INITIALIZATION_VECTOR.slice(0, 4),
    0,
    0,
    blockLength,
    flags,
  ];
  let message = Array.from({ length: 16 }, (_, index) => block.readUInt32LE(index * 4));

  for (let index = 0; index < 7; index += 1) {
    round(state, message);
    message = MESSAGE_PERMUTATION.map((messageIndex) => message[messageIndex]);
  }

  return [
    ...Array.from({ length: 8 }, (_, index) => (state[index] ^ state[index + 8]) >>> 0),
    ...Array.from(
      { length: 8 },
      (_, index) => (state[index + 8] ^ chainingValue[index]) >>> 0,
    ),
  ];
}

function blake3OneChunk(input) {
  assert.ok(input.length <= 1024, "fixture input exceeds one BLAKE3 chunk");

  let chainingValue = [...INITIALIZATION_VECTOR];
  let offset = 0;
  let compressedBlocks = 0;

  while (input.length - offset > 64) {
    chainingValue = compress(
      chainingValue,
      input.subarray(offset, offset + 64),
      64,
      compressedBlocks === 0 ? CHUNK_START : 0,
    ).slice(0, 8);
    offset += 64;
    compressedBlocks += 1;
  }

  const finalBlock = Buffer.alloc(64);
  input.copy(finalBlock, 0, offset);
  const output = compress(
    chainingValue,
    finalBlock,
    input.length - offset,
    CHUNK_END | (compressedBlocks === 0 ? CHUNK_START : 0) | ROOT,
  );
  const hash = Buffer.alloc(32);
  output.slice(0, 8).forEach((word, index) => hash.writeUInt32LE(word, index * 4));
  return hash;
}

function asBigInt(value) {
  assert.equal(typeof value, "bigint", "CBOR integers must remain BigInt");
  return value;
}

function encodeArgument(major, value) {
  const integer = asBigInt(value);
  assert.ok(integer >= 0n && integer <= U64_MAX, "CBOR argument must fit u64");
  if (integer < 24n) {
    return Buffer.from([Number((BigInt(major) << 5n) | integer)]);
  }
  if (integer <= 0xffn) {
    return Buffer.from([(major << 5) | 24, Number(integer)]);
  }
  if (integer <= 0xffffn) {
    const output = Buffer.allocUnsafe(3);
    output[0] = (major << 5) | 25;
    output.writeUInt16BE(Number(integer), 1);
    return output;
  }
  if (integer <= 0xffff_ffffn) {
    const output = Buffer.allocUnsafe(5);
    output[0] = (major << 5) | 26;
    output.writeUInt32BE(Number(integer), 1);
    return output;
  }
  const output = Buffer.allocUnsafe(9);
  output[0] = (major << 5) | 27;
  output.writeBigUInt64BE(integer, 1);
  return output;
}

function encodeCanonical(value) {
  if (value === null) {
    return Buffer.from([0xf6]);
  }
  if (Buffer.isBuffer(value)) {
    return Buffer.concat([encodeArgument(2, BigInt(value.length)), value]);
  }
  if (typeof value === "string") {
    assert.equal(value, value.normalize("NFC"), "text strings must already be NFC");
    const utf8 = Buffer.from(value, "utf8");
    return Buffer.concat([encodeArgument(3, BigInt(utf8.length)), utf8]);
  }
  if (Array.isArray(value)) {
    return Buffer.concat([encodeArgument(4, BigInt(value.length)), ...value.map(encodeCanonical)]);
  }
  if (value instanceof Map) {
    const entries = [...value.entries()].map(([key, entryValue]) => ({
      key: encodeCanonical(key),
      value: encodeCanonical(entryValue),
    }));
    entries.sort((left, right) => Buffer.compare(left.key, right.key));
    for (let index = 1; index < entries.length; index += 1) {
      assert.notEqual(
        Buffer.compare(entries[index - 1].key, entries[index].key),
        0,
        "CBOR map has duplicate keys",
      );
    }
    return Buffer.concat([
      encodeArgument(5, BigInt(entries.length)),
      ...entries.flatMap((entry) => [entry.key, entry.value]),
    ]);
  }
  if (typeof value === "bigint") {
    const integer = asBigInt(value);
    return integer >= 0n
      ? encodeArgument(0, integer)
      : encodeArgument(1, -1n - integer);
  }
  assert.fail(`unsupported CBOR semantic value: ${typeof value}`);
}

function decodeCanonical(bytes) {
  let offset = 0;
  const value = decodeItem();
  assert.equal(offset, bytes.length, "CBOR has trailing bytes");
  return value;

  function read(count) {
    assert.ok(offset + count <= bytes.length, "CBOR is truncated");
    const value = bytes.subarray(offset, offset + count);
    offset += count;
    return value;
  }

  function safeLength(value) {
    assert.ok(value <= BigInt(Number.MAX_SAFE_INTEGER), "CBOR length exceeds JS bounds");
    return Number(value);
  }

  function readArgument(additional) {
    assert.notEqual(additional, 31, "indefinite CBOR lengths are forbidden");
    if (additional < 24) {
      return BigInt(additional);
    }
    if (additional === 24) {
      return BigInt(read(1)[0]);
    }
    if (additional === 25) {
      return BigInt(read(2).readUInt16BE(0));
    }
    if (additional === 26) {
      return BigInt(read(4).readUInt32BE(0));
    }
    if (additional === 27) {
      return read(8).readBigUInt64BE(0);
    }
    assert.fail("reserved CBOR additional-information value");
  }

  function decodeItem() {
    const first = read(1)[0];
    const major = first >>> 5;
    const additional = first & 0x1f;
    if (major === 0) {
      return readArgument(additional);
    }
    if (major === 1) {
      return -1n - readArgument(additional);
    }
    if (major === 2) {
      return Buffer.from(read(safeLength(readArgument(additional))));
    }
    if (major === 3) {
      const utf8 = read(safeLength(readArgument(additional)));
      const text = utf8.toString("utf8");
      assert.ok(Buffer.from(text, "utf8").equals(utf8), "CBOR text is not valid UTF-8");
      return text;
    }
    if (major === 4) {
      return Array.from({ length: safeLength(readArgument(additional)) }, decodeItem);
    }
    if (major === 5) {
      const map = new Map();
      const keyEncodings = new Set();
      for (let index = 0; index < safeLength(readArgument(additional)); index += 1) {
        const key = decodeItem();
        const encodedKey = encodeCanonical(key).toString("hex");
        assert.ok(!keyEncodings.has(encodedKey), "CBOR map has duplicate keys");
        keyEncodings.add(encodedKey);
        map.set(key, decodeItem());
      }
      return map;
    }
    if (major === 7 && additional === 22) {
      return null;
    }
    assert.fail("tags, floats, undefined, and unsupported simple values are forbidden");
  }
}

function assertNfc(value) {
  if (typeof value === "string") {
    assert.equal(value, value.normalize("NFC"), "text strings must already be NFC");
  } else if (Array.isArray(value)) {
    value.forEach(assertNfc);
  } else if (value instanceof Map) {
    value.forEach((mapValue, key) => {
      assertNfc(key);
      assertNfc(mapValue);
    });
  }
}

function expectNumericMap(value, keys, label) {
  assert.ok(value instanceof Map, `${label}: map`);
  assert.deepEqual(
    [...value.keys()].sort((left, right) => (left < right ? -1 : left > right ? 1 : 0)),
    keys,
    `${label}: exact keys`,
  );
}

function numericKeys(length) {
  return Array.from({ length }, (_, index) => BigInt(index));
}

function expectBytes(value, length, label) {
  assert.ok(Buffer.isBuffer(value), `${label}: byte string`);
  assert.equal(value.length, length, `${label}: byte length`);
}

function assertEnvelopeSemantics(envelope, vector) {
  expectNumericMap(envelope, numericKeys(10), `${vector.name}: unsigned envelope`);
  assert.equal(envelope.get(0n), 1n, `${vector.name}: protocol version`);
  const kind = envelope.get(1n);
  assert.ok(kind > 0n && kind <= 0xffffn, `${vector.name}: operation kind`);

  const scope = envelope.get(2n);
  expectNumericMap(scope, [0n, 1n, 2n], `${vector.name}: scope`);
  expectBytes(scope.get(0n), 32, `${vector.name}: verse id`);
  if (scope.get(1n) !== null) expectBytes(scope.get(1n), 32, `${vector.name}: petal id`);
  if (scope.get(2n) !== null) expectBytes(scope.get(2n), 32, `${vector.name}: resource id`);
  assert.ok(scope.get(1n) !== null || scope.get(2n) === null, `${vector.name}: resource needs petal`);

  const author = envelope.get(3n);
  expectNumericMap(author, [0n, 1n], `${vector.name}: author`);
  assert.equal(author.get(0n), fixture.signing_key.did, `${vector.name}: canonical DID`);
  expectBytes(author.get(1n), 32, `${vector.name}: author public key`);
  assert.ok(
    author.get(1n).equals(Buffer.from(fixture.signing_key.public_key_hex, "hex")),
    `${vector.name}: DID/public-key binding fixture`,
  );

  const capability = envelope.get(4n);
  expectNumericMap(capability, [0n, 1n], `${vector.name}: capability`);
  expectBytes(capability.get(0n), 32, `${vector.name}: capability chain id`);
  assert.ok(asBigInt(capability.get(1n)) >= 0n, `${vector.name}: scope epoch`);
  expectBytes(envelope.get(5n), 32, `${vector.name}: schema hash`);
  expectBytes(envelope.get(6n), 32, `${vector.name}: branch id`);

  const parents = envelope.get(7n);
  assert.ok(Array.isArray(parents), `${vector.name}: parent array`);
  parents.forEach((parent, index) => {
    expectBytes(parent, 32, `${vector.name}: parent ${index}`);
    if (index > 0) {
      assert.ok(Buffer.compare(parents[index - 1], parent) < 0, `${vector.name}: sorted unique parents`);
    }
  });

  const hlc = envelope.get(8n);
  expectNumericMap(hlc, [0n, 1n], `${vector.name}: HLC`);
  assert.ok(asBigInt(hlc.get(0n)) >= 0n, `${vector.name}: HLC wall time`);
  assert.ok(asBigInt(hlc.get(1n)) >= 0n && asBigInt(hlc.get(1n)) <= 0xffff_ffffn, `${vector.name}: HLC counter`);

  const payload = envelope.get(9n);
  expectNumericMap(payload, [0n, 1n, 2n], `${vector.name}: payload reference`);
  expectBytes(payload.get(0n), 32, `${vector.name}: ciphertext hash`);
  assert.ok(asBigInt(payload.get(1n)) >= 0n, `${vector.name}: ciphertext length`);
  if (payload.get(2n) === null) {
    assert.equal(asBigInt(payload.get(1n)), 0n, `${vector.name}: empty payload length`);
    assert.ok(
      payload.get(0n).equals(blake3OneChunk(Buffer.alloc(0))),
      `${vector.name}: empty payload hash`,
    );
  } else {
    expectNumericMap(payload.get(2n), [0n, 1n, 2n], `${vector.name}: encryption`);
    assert.ok(asBigInt(payload.get(2n).get(0n)) > 0n, `${vector.name}: suite id`);
    expectBytes(payload.get(2n).get(1n), 32, `${vector.name}: scope key id`);
    assert.ok(Buffer.isBuffer(payload.get(2n).get(2n)), `${vector.name}: nonce`);
  }

  if (kind === 2n) {
    assert.equal(parents.length, 0, `${vector.name}: branch_genesis has no parents`);
  }
  if (kind === 3n) {
    assert.ok(parents.length >= 2, `${vector.name}: merge has multiple parents`);
    assert.equal(payload.get(2n), null, `${vector.name}: no-payload merge`);
  }
  if (kind === 4n) {
    assert.equal(parents.length, 1, `${vector.name}: epoch bump has one parent`);
    assert.equal(payload.get(2n), null, `${vector.name}: payload-free epoch bump`);
  }
}

function completeEnvelopeFrom(unsignedEnvelope, signature) {
  const complete = new Map();
  for (let key = 0n; key <= 9n; key += 1n) {
    complete.set(key, unsignedEnvelope.get(key));
  }
  complete.set(10n, signature);
  return complete;
}

function payloadAadEnvelopeFrom(unsignedEnvelope) {
  const payload = unsignedEnvelope.get(9n);
  assert.notEqual(payload.get(2n), null, "payload AAD requires encryption metadata");
  const aadEnvelope = new Map();
  for (let key = 0n; key <= 8n; key += 1n) {
    aadEnvelope.set(key, unsignedEnvelope.get(key));
  }
  aadEnvelope.set(9n, new Map([[2n, payload.get(2n)]]));
  return aadEnvelope;
}

const publicKey = createPublicKey({
  key: Buffer.concat([
    Buffer.from("302a300506032b6570032100", "hex"),
    Buffer.from(fixture.signing_key.public_key_hex, "hex"),
  ]),
  format: "der",
  type: "spki",
});

test("operation-envelope v1 vectors independently re-derive canonical artifacts", () => {
  const domain = Buffer.from(fixture.signing_key.signature_domain_hex, "hex");

  for (const vector of fixture.vectors) {
    const unsignedEnvelope = Buffer.from(vector.unsigned_envelope_cbor_hex, "hex");
    const completeEnvelope = Buffer.from(vector.complete_envelope_cbor_hex, "hex");
    const signature = Buffer.from(vector.signature_hex, "hex");
    const ciphertext = Buffer.from(vector.ciphertext_hex, "hex");

    const unsignedSemantic = decodeCanonical(unsignedEnvelope);
    assertNfc(unsignedSemantic);
    assertEnvelopeSemantics(unsignedSemantic, vector);
    const rederivedUnsigned = encodeCanonical(unsignedSemantic);
    assert.ok(
      rederivedUnsigned.equals(unsignedEnvelope),
      `${vector.name}: unsigned envelope has canonical key order and minimal integers`,
    );

    const completeSemantic = decodeCanonical(completeEnvelope);
    assertNfc(completeSemantic);
    expectNumericMap(completeSemantic, numericKeys(11), `${vector.name}: complete envelope`);
    expectBytes(completeSemantic.get(10n), 64, `${vector.name}: signature field`);
    assert.ok(completeSemantic.get(10n).equals(signature), `${vector.name}: stored signature`);
    const rederivedComplete = encodeCanonical(completeSemantic);
    assert.ok(
      rederivedComplete.equals(completeEnvelope),
      `${vector.name}: complete envelope has canonical key order and minimal integers`,
    );
    const reconstructedComplete = encodeCanonical(completeEnvelopeFrom(unsignedSemantic, signature));
    assert.ok(
      reconstructedComplete.equals(completeEnvelope),
      `${vector.name}: complete envelope reconstructs from semantic fields`,
    );

    assert.ok(
      verify(null, Buffer.concat([domain, rederivedUnsigned]), publicKey, signature),
      `${vector.name}: Ed25519 signature over re-derived unsigned envelope`,
    );
    assert.equal(
      blake3OneChunk(reconstructedComplete).toString("hex"),
      vector.op_id_hex,
      `${vector.name}: signed-artifact operation ID`,
    );
    assert.equal(
      blake3OneChunk(ciphertext).toString("hex"),
      vector.ciphertext_hash_hex,
      `${vector.name}: ciphertext commitment`,
    );

    if (vector.payload_plaintext_cbor_hex) {
      const plaintext = Buffer.from(vector.payload_plaintext_cbor_hex, "hex");
      const payloadSemantic = decodeCanonical(plaintext);
      assertNfc(payloadSemantic);
      assert.ok(
        encodeCanonical(payloadSemantic).equals(plaintext),
        `${vector.name}: payload has canonical key order, NFC text, and minimal integers`,
      );
      assert.equal(
        blake3OneChunk(plaintext).toString("hex"),
        vector.payload_plaintext_hash_hex,
        `${vector.name}: canonical quantized payload`,
      );
    }

    if (vector.payload_aad_envelope_cbor_hex) {
      const storedAadEnvelope = Buffer.from(vector.payload_aad_envelope_cbor_hex, "hex");
      const aadSemantic = decodeCanonical(storedAadEnvelope);
      assertNfc(aadSemantic);
      const rederivedAadEnvelope = encodeCanonical(payloadAadEnvelopeFrom(unsignedSemantic));
      assert.ok(
        encodeCanonical(aadSemantic).equals(storedAadEnvelope),
        `${vector.name}: stored payload AAD envelope is canonical`,
      );
      assert.ok(
        rederivedAadEnvelope.equals(storedAadEnvelope),
        `${vector.name}: payload AAD omits only ciphertext hash and length`,
      );
      assert.equal(
        Buffer.concat([Buffer.from("fe-oplog-payload-aad-v1\0", "ascii"), rederivedAadEnvelope]).toString("hex"),
        vector.payload_aad_preimage_hex,
        `${vector.name}: payload AAD preimage`,
      );
    }
  }
});
