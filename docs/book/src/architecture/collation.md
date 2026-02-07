# CouchDB Collation

CouchDB defines a specific ordering for JSON values that differs from naive
lexicographic comparison of serialized JSON strings. This ordering is inherited
from Erlang's term comparison and is used everywhere keys are compared: view
indexes, `_all_docs` key ranges, Mango query evaluation, and internal storage
engine key encoding.

RouchDB implements this ordering in `rouchdb-core/src/collation.rs`.

## Type Ordering

Different JSON types sort in this fixed order, from lowest to highest:

```
null  <  boolean  <  number  <  string  <  array  <  object
```

A `null` value is always less than `false`, which is always less than `-1000`,
which is always less than `""`, and so on. The type boundary is absolute --
there is no number large enough to sort after even the empty string.

Internally, each type is assigned a numeric rank:

| JSON Type | Rank |
|-----------|------|
| `null`    | 1    |
| `boolean` | 2    |
| `number`  | 3    |
| `string`  | 4    |
| `array`   | 5    |
| `object`  | 6    |

When two values have different ranks, the comparison is immediate. Same-rank
values are compared with type-specific rules described below.

## Within-Type Comparison Rules

### Null

All nulls are equal.

### Boolean

`false < true`.

### Number

Compared by numeric value as IEEE 754 f64. `-100 < -1 < 0 < 1 < 1.5 < 2`.

### String

Standard lexicographic (Unicode codepoint) ordering. `"a" < "aa" < "b"`.

### Array

Element-by-element using recursive `collate`. If all shared elements are equal,
the shorter array sorts first.

```
[]       < [1]
[1]      < [2]
[1]      < [1, 2]
[1, "a"] < [1, "b"]
```

### Object

Keys are sorted alphabetically first, then compared key-by-key. For each key
pair, the key strings are compared; if equal, the values are compared
recursively. If all shared key-value pairs are equal, the object with fewer keys
sorts first.

```
{}           < {"a": 1}
{"a": 1}     < {"a": 2}
{"a": 1}     < {"b": 1}
{"a": 1}     < {"a": 1, "b": 2}
```

## The `collate` Function

```rust
pub fn collate(a: &Value, b: &Value) -> Ordering
```

This is the primary comparison entry point. It first compares type ranks, then
delegates to type-specific comparison. It can be used anywhere you need
CouchDB-compatible ordering of arbitrary JSON values.

### Usage Examples

```rust
use serde_json::json;
use rouchdb_core::collation::collate;
use std::cmp::Ordering;

// Cross-type: null < number
assert_eq!(collate(&json!(null), &json!(42)), Ordering::Less);

// Cross-type: number < string
assert_eq!(collate(&json!(9999), &json!("")), Ordering::Less);

// Same type: numeric comparison
assert_eq!(collate(&json!(-1), &json!(0)), Ordering::Less);

// Same type: array element-by-element
assert_eq!(collate(&json!([1, 2]), &json!([1, 3])), Ordering::Less);
```

## Indexable String Encoding

Storage engines like redb store keys as byte arrays and compare them
lexicographically. To preserve CouchDB collation order in a byte-ordered
key-value store, JSON values must be encoded into strings that sort
lexicographically in the same order as `collate`.

```rust
pub fn to_indexable_string(v: &Value) -> String
```

### Encoding Scheme

Each value is encoded with a type-prefix character that preserves the
cross-type ordering:

| Type    | Prefix | Encoding |
|---------|--------|----------|
| Null    | `1`    | Just the prefix character |
| Boolean | `2`    | `2F` for false, `2T` for true |
| Number  | `3`    | `3` + encoded number (see below) |
| String  | `4`    | `4` + the raw string value |
| Array   | `5`    | `5` + encoded elements separated by null bytes (`\0`) |
| Object  | `6`    | `6` + sorted key-value pairs separated by null bytes |

Because the prefix characters are `1` through `6`, the inter-type ordering is
automatically correct: any null-encoded string (`"1..."`) sorts before any
boolean-encoded string (`"2..."`), and so on.

### Number Encoding

Numbers require special treatment because naive string representations do not
sort correctly (`"9" > "10"` lexicographically). The encoding uses a scheme
matching PouchDB's `numToIndexableString`:

**Zero:** Encoded as `"1"`.

**Positive numbers:** Prefix `"2"`, followed by a 5-digit zero-padded
exponent (offset by 10000 to keep it positive), followed by the mantissa
(normalized to `[1, 10)`).

```
encode(1)    -> "3" + "2" + "10000" + "1."
encode(100)  -> "3" + "2" + "10002" + "1."
encode(1.5)  -> "3" + "2" + "10000" + "1.5"
```

Since the exponent field is fixed-width and the mantissa is a decimal in
`[1, 10)`, larger numbers always produce lexicographically later strings.

**Negative numbers:** Prefix `"0"`, followed by the _inverted_ exponent
(`10000 - exponent`), followed by the _inverted_ mantissa (`10 - mantissa`).
Inversion ensures that numbers closer to zero (larger negatives) sort after
more-negative numbers.

```
encode(-1)   -> "3" + "0" + "10000" + "9."
encode(-100) -> "3" + "0" + "09998" + "9."
```

The full encoding for a number is: `"3"` (type prefix) + sign/magnitude
encoding.

The ordering is:

```
-100      ->  "3" + "0" + "09998..."   (sorts first)
-1        ->  "3" + "0" + "10000..."
 0        ->  "3" + "1"
 1        ->  "3" + "2" + "10000..."
 100      ->  "3" + "2" + "10002..."   (sorts last)
```

### Array and Object Encoding

Arrays encode each element recursively with null-byte separators. Because
`\0` is the lowest byte value, a shorter array with a matching prefix will
always sort before a longer one.

Objects sort their keys alphabetically, then encode alternating key-value
pairs separated by null bytes.

## Why This Matters

### Views

Map/reduce views emit keys that are stored in sorted order. The storage
engine must compare these keys in CouchDB collation order. By encoding
them with `to_indexable_string`, ordinary byte-level comparison produces the
correct ordering.

### `_all_docs` Key Ranges

The `startkey`/`endkey` parameters on `_all_docs` use CouchDB collation. The
adapter encodes the boundary values and performs byte-range scans.

### Mango Queries

Mango `$gt`, `$gte`, `$lt`, `$lte` operators compare values according to
CouchDB collation. The `rouchdb-query` crate uses `collate` for these
comparisons.

### Replication Correctness

If two replicas sort the same view index differently, they will produce
different results for the same query. CouchDB collation ensures all replicas
agree on ordering.

## Verifying Correctness

The test suite confirms that `to_indexable_string` preserves `collate`
ordering across the full type spectrum:

```rust
let values = vec![
    json!(null), json!(false), json!(true),
    json!(0), json!(1), json!(100),
    json!("a"), json!("b"),
    json!([]), json!({}),
];
let encoded: Vec<String> = values.iter().map(to_indexable_string).collect();

for i in 0..encoded.len() {
    for j in (i + 1)..encoded.len() {
        assert!(encoded[i] < encoded[j]);
    }
}
```

The negative-number tests further verify that `-100 < -1 < 0` is preserved
in the encoded representation.
