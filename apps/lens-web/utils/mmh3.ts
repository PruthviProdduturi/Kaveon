/**
 * MurmurHash3 (MMH3) Hash64 Implementation
 *
 * This implementation computes the 64-bit MurmurHash3 hash of a string,
 * matching the behavior of the C# implementation used for dimension keys:
 * - Converts input to lowercase
 * - Trims whitespace
 * - Uses MurmurHash3 x64 128-bit algorithm
 * - Returns first 64 bits as signed integer
 *
 * C# Reference:
 * var murmurHash = MurmurHash3Factory.Instance.Create(new MurmurHash3Config { HashSizeInBits = 128 });
 * input = input.ToLower().Trim();
 * var hashValue = murmurHash.ComputeHash(Encoding.UTF8.GetBytes(input));
 * byte[] hashBytes = hashValue.Hash;
 * long result = BitConverter.ToInt64(hashBytes, 0);
 *
 * Used to generate dimension key values from display values for efficient
 * fact table filtering without requiring dimension table lookups.
 */

import murmur from 'murmurhash3js-revisited';

/**
 * MurmurHash3 128-bit hash implementation (returns first 64 bits)
 *
 * @param str - Input string to hash
 * @param seed - Hash seed (default: 0)
 * @returns 64-bit hash as a string (to match C# behavior)
 */
export function mmh3Hash64(str: string, seed: number = 0): string {
  // Preprocess: lowercase and trim (matching C# implementation)
  const processedStr = str.toLowerCase().trim();

  // Convert string to UTF-8 bytes
  const encoder = new TextEncoder();
  const bytes = encoder.encode(processedStr);

  // Use x64 128-bit variant (matches C# MurmurHash3Config with HashSizeInBits = 128)
  const hash = murmur.x64.hash128(bytes, seed);

  // The hash is returned as a hex string (32 chars = 128 bits)
  // Take first 16 hex chars (64 bits)
  const first64BitsHex = hash.substring(0, 16);

  // Convert to BigInt and then to signed 64-bit integer
  const unsignedValue = BigInt('0x' + first64BitsHex);

  // Convert to signed 64-bit integer
  const signed64 = unsignedValue > 0x7fffffffffffffffn
    ? unsignedValue - 0x10000000000000000n
    : unsignedValue;

  return signed64.toString();
}

/**
 * Test function to verify hash computation matches C# implementation
 */
export function testMmh3Hash64(): void {
  const testCases = [
    { input: 'AllUp', expected: '-5636343888969758749' },
    { input: 'M365 Copilot All Up', expected: '4999588089921664978' },
    { input: '', expected: '0' },
  ];

  console.log('Testing MMH3 Hash64 implementation (x64 128-bit variant, toLowerCase + trim):');
  testCases.forEach(({ input, expected }) => {
    const result = mmh3Hash64(input);
    const match = result === expected;
    console.log(`  "${input}" -> ${result} ${match ? '✓' : `✗ (expected ${expected})`}`);
  });
}
