# unicode-math

This is a fork of [https://github.com/s3bk/unicode-math/](https://github.com/s3bk/unicode-math/)

The data for symbols is constructed by the build script from two sources:

   - `unicode-math-table.tex` file from `unicode-math` (<https://github.com/latex3/unicode-math>), under the The LaTeX Project Public License 1.3c.
   - `unimathsymbols.txt` file from `LaTeX Unicode Character References` (<https://milde.users.sourceforge.net/LUCR/Math/>), under the The LaTeX Project Public License 1.3c or any later license.

The data for the mathematical alphanumeric replacement table comes from [https://www.unicode.org/charts/nameslist/](https://www.unicode.org/charts/nameslist/), more precisely the file [https://www.unicode.org/charts/nameslist/n_1D400.html](https://www.unicode.org/charts/nameslist/n_1D400.html).

## Drafft.ink

This copy is published as `drafftink-unicode-math` so that `drafftink-rex` (a vendored ReX) can depend on it from
crates.io. The library name is still `unicode_math`. The crate code is MIT; the data tables in `resources/` are under
the LaTeX Project Public License 1.3c, as described above.
