# Font Layout

+ Font
  + symbols.rs
    - Mapping from TeX Commands to GlyphID and Type
  + metrics.rs
    - Mapping from GlyphID to Glyph metrics
  + variants.rs
    - Collection of variant glyph construction
  + kernings.rs
    - Collection of kernings data
  + input.rs
    - Mapping from Unicode input to GlyphID and Type
  + styles.rs
    - An ID to ID mapping encoding style changes.
  + constants.rs
    - MathTable constants
    - Font scaling (pointsPerEm)

# Additional considerations
  - Style Changes
    - State handled in parser.
    - Requires an ID -> ID mapping. (Parsed from unicode-math?)
    - Applied post symbols.rs, input.rs
  - Size changes
    - Handled by simply scaling?


# Font Selection

The current supported family and weights for fonts is shown
in the following table, which shows the corresponding unicode
ranges for each weight and family.

|                  | LATIN | latin | GREEK | greek | digit |
|------------------|-------|-------|-------|-------|-------|
| Bold (Serif)     | 1D400 | 1D41A | 1D6A8 | 1D6C2 | 1D7CE |
| Italic           | 1D434 | 1D44E | 1D6E2 | 1D6FC |       |
| Bold Italic      | 1D468 | 1D482 | 1D71C | 1D736 |       |
| Script           | 1D49C | 1D4B6 |       |       |       |
| Bold Script      | 1D4D0 | 1D4EA |       |       |       |
| Fraktur          | 1D504 | 1D51E |       |       |       |
| Blackboard       | 1D538 | 1D552 |       |       | 1D7D8 |
| Bold Fraktur     | 1D56C | 1D586 |       |       |       |
| Sans Serif       | 1D5A0 | 1D5BA |       |       | 1D7E2 |
| Bold Sans Serif  | 1D5D4 | 1D5EE | 1D756 | 1D770 | 1D7EC |
| It Sans Serif    | 1D608 | 1D622 |       |       |       |
| Bd It Sans Serif | 1D63C | 1D656 | 1D790 | 1D7AA |       |
| Monospace        | 1D670 | 1D68A |       |       | 1D7F6 |

**TODO**: We should extend this to included the non-standard
family-weight variants found in STIX.  Reference
[unicode-math-usv.dtx](unicode-math-usv) for more details.

|                    | LATIN | latin | GREEK | greek | digit |
|--------------------|-------|-------|-------|-------|-------|
| It Sans Serif      |       |       | E1BF  | E1D8  | E1B4  |
| It Blackboard      | E154  | E166  |       |       |       |
| Calogriphic        | E22D  |       |       |       | E262  |
| It Slash Sans      | E294  | E2C8  |       | E32C  |       |
| Bold Blackboard    | E38A  | E39D  |       |       |       |
| Bold Sans Roman    |       |       |       |       | E3B7  |
| Bold It Blackboard | E200  | E213  |       |       |       |
| Bold Cali          | E247  |       |       |       |       |
| Bold It Slash      | E295  | E2C9  |       | E32D  |       |

[unicode-math-usv]: (http://mirror.utexas.edu/ctan/macros/latex/contrib/unicode-math/unicode-math-usv.dtx)
