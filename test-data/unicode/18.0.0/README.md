# Unicode 18.0 test data

The files below are official Unicode 18.0.0 data files downloaded from
<https://www.unicode.org/Public/18.0.0/>. They are used as offline conformance
corpora for grapheme segmentation, default-ignorable property data, and emoji
recognition.

| File | Source path | SHA-256 |
| --- | --- | --- |
| `DerivedCoreProperties.txt` | `ucd/DerivedCoreProperties.txt` | `09c928886a178fcafd93c29e4bd59073a058e5a100b716d425cb563ab50f68c9` |
| `GraphemeBreakTest.txt` | `ucd/auxiliary/GraphemeBreakTest.txt` | `b0cf047ee94485bbdc846de2b902f5f8a815f6b674f9d04223cddadd91c9df31` |
| `emoji-test.txt` | `emoji/emoji-test.txt` | `8f3735cda1f92a779d78af67cf86066bb1f07143dc22f2ac29394d9bc57ab21a` |
| `emoji-sequences.txt` | `emoji/emoji-sequences.txt` | `1823dce71f3dd9cb0ad1976797baee4ffbdc1909756f43af8ce4c57f732843a4` |
| `emoji-zwj-sequences.txt` | `emoji/emoji-zwj-sequences.txt` | `f61b5213bdf85a57741a9f903cb38d0f39a3d13be6ff4aca3b39857e859a9191` |

The ICU4X property test exhaustively compares `Default_Ignorable_Code_Point`
with the UCD over every Unicode scalar value. The segmenter test checks every
case in `GraphemeBreakTest.txt` and reports the legacy implementation's
differences without weakening the ICU conformance assertion.

Unicode data is provided under the Unicode Terms of Use and Unicode Data Files
and Software License: <https://www.unicode.org/terms_of_use.html>.
