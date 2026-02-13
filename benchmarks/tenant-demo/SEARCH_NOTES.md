# Search Architecture Notes

## 1. Partial and Fuzzy Matching

### Why full words are required today

pg_textsearch uses PostgreSQL's text search configuration (e.g. `english`) to
tokenize both documents and queries via `to_tsvector`. The term dictionary uses
**exact lexeme matching** — binary search in segments, hash lookup in the
memtable. Typing `"datab"` produces the lexeme `"datab"`, which doesn't match
`"databas"` (the stem of `"database"`).

Stemming helps with word forms (`"running"` matches `"run"`, `"databases"`
matches `"database"`), but not with incomplete input.

### Prefix matching

Would allow typing `"algo"` to match `"algorithm"`. The segment term
dictionary is sorted, so a prefix scan is structurally feasible: binary-search
to the first matching term, then iterate forward. The memtable string table
(dshash) doesn't support prefix lookups natively since it's hash-based.

BM25 scoring becomes ambiguous when a prefix expands to multiple terms — which
IDF do you use?

### Fuzzy matching (typo tolerance)

Requires edit-distance (Levenshtein) or n-gram similarity. Not compatible with
the current term dictionary design, which is optimized for exact key lookup.
Common approaches include trigram indexes (`pg_trgm`) or a spell-correction
layer that rewrites queries before they hit BM25.

---

## 2. Comparison with Elasticsearch (Lucene)

### Term index: FST vs sorted array

The biggest structural difference is Lucene's **Finite State Transducer (FST)**
term index — a compressed trie that maps term prefixes to blocks in the term
dictionary.

```
          root
         / | \
        a  d  r
       /   |   \
      l    a    u
     /     |     \
   go    tab     n
   |      |
  rithm  ase
```

The FST gives Lucene three capabilities pg_textsearch doesn't have today:

1. **Prefix queries** — walk the FST to `"algo"` and enumerate descendants.
2. **Fuzzy queries** — intersect a Levenshtein automaton with the FST to find
   all terms within edit distance N in a single pass.
3. **Compact in-memory representation** — shared prefixes and suffixes compress
   well, keeping the index in memory even for millions of terms.

### Segment architecture (very similar)

| Concept              | Lucene                          | pg_textsearch              |
|----------------------|---------------------------------|----------------------------|
| In-memory buffer     | In-memory segment               | Memtable                   |
| Immutable disk files | Segments (.tip, .tim, .doc)     | V2 segments in PG pages    |
| Background compaction| Tiered merge policy             | Level-based compaction     |
| Term lookup          | FST -> term dict -> postings    | Binary search -> postings  |
| Skip lists           | Multi-level skip data in .doc   | Skip entries in blocks     |
| Block scoring        | Block-max WAND (since Lucene 8) | Block-max WAND             |

### How Elasticsearch exposes this

- **`match` query** — tokenize + stem, exact lexeme lookup (same as
  pg_textsearch today)
- **`prefix` query** — FST prefix walk
- **`fuzzy` query** — Levenshtein automaton intersected with FST (default edit
  distance 2)
- **`match` with `fuzziness: "AUTO"`** — tokenize, then fuzzy-expand each term
- **`search_as_you_type` field type** — edge-ngram sub-fields at index time
  (e.g. `"algorithm"` -> `"a"`, `"al"`, `"alg"`, ...) for instant prefix
  matching

### Possible paths for pg_textsearch

1. **FST term index** — replace the sorted term array with an FST. The
   "proper" solution but a significant undertaking (FST construction,
   serialization, automaton intersection).
2. **Edge n-gram approach** (like ES's `search_as_you_type`) — at index time,
   emit additional lexemes for prefixes. Simpler to implement within the
   current architecture but increases index size.

---

## 3. Improving Indexed Tokens from OCR (Tesseract) Output

### What PostgreSQL's text search config already does

With `text_config='english'`, `to_tsvector` applies:

1. **Parser** — splits text into tokens (words, numbers, emails, URLs)
2. **Stop words** — removes "the", "is", "and", etc.
3. **Snowball stemmer** — `"databases"` -> `"databas"`, `"running"` -> `"run"`

So even raw Tesseract output gets stemmed. But stemming can't fix garbled
input.

### OCR-specific problems

| Problem             | Example                  | Effect on search                |
|---------------------|--------------------------|---------------------------------|
| Broken words        | `"algo rithm"`           | Two useless lexemes             |
| Merged words        | `"thedatabase"`          | One unrecognizable lexeme       |
| Misrecognized chars | `"datab4se"`, `"aIgorithm"` | Won't stem correctly         |
| Stray punctuation   | `"data.base"`, `"algo—rithm"` | May split or fail to match |

### Recommended approaches

**1. Pre-process before storing (highest impact)**

Clean OCR text before inserting into the `content` column:

- Rejoin hyphenated/broken words (`"algo- rithm"` -> `"algorithm"`)
- Normalize Unicode confusables (`"fi"` ligature -> `"fi"`, `"—"` -> `"-"`)
- Strip stray non-alpha characters inside words
- Spell-check correction (e.g. hunspell dictionary to fix `"datab4se"` ->
  `"database"`)

This is the highest-ROI step. Garbage in, garbage out.

**2. Custom text search dictionary**

PostgreSQL allows chaining dictionaries in a text search configuration:

```sql
-- Map common OCR errors to correct forms
CREATE TEXT SEARCH DICTIONARY ocr_synonyms (
    TEMPLATE = synonym,
    SYNONYMS = ocr_fixes
);

CREATE TEXT SEARCH CONFIGURATION ocr_english (COPY = english);
ALTER TEXT SEARCH CONFIGURATION ocr_english
    ALTER MAPPING FOR asciiword
    WITH ocr_synonyms, english_stem;
```

Then use `text_config='ocr_english'` on the BM25 index. This maps known OCR
errors to correct lexemes at index time without changing the stored text.

**3. Edge n-grams (for search-as-you-type)**

A pg_textsearch feature that would emit prefixes of each lexeme at index time.
For `"algorithm"`, also index `"alg"`, `"algo"`, `"algor"`, etc. Multiplies
index size but enables prefix matching without an FST.

Orthogonal to OCR quality — helps with partial typing regardless of token
source, but only useful on top of clean data.
