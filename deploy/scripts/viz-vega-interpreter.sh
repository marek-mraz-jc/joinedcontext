#!/bin/sh
# vega-interpreter 2.x ships only an ES module importing the bare name 'vega-util', which a page
# loads only through an import map, and an inline map needs a CSP hash the renderer does not give.
# This writes the same code as a classic script over the vega global, which carries every
# vega-util export: the one import becomes a read of `vega`, the export an assignment to
# `vega.expressionInterpreter`, the name vega-embed asks for with `ast: true`.
#   npm pack vega-interpreter@2.3.2 && tar xzf vega-interpreter-2.3.2.tgz
#   scripts/viz-vega-interpreter.sh package/build/vega-interpreter.js > images/viz/vendor/vega-interpreter-2.3.2.js
set -eu
src=$1
grep -qx "import { ascending, isString, DisallowedObjectProperties } from 'vega-util';" "$src"
grep -qx "export { expression as expressionInterpreter };" "$src"
printf '(function (vega) {\n"use strict";\n'
sed -e "s/^import { ascending, isString, DisallowedObjectProperties } from 'vega-util';\$/const { ascending, isString, DisallowedObjectProperties } = vega;/" \
    -e 's/^export { expression as expressionInterpreter };$/vega.expressionInterpreter = expression;/' \
    -e '/^\/\/# sourceMappingURL=/d' "$src"
printf '})(self.vega);\n'
