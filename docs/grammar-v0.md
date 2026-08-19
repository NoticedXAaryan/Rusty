# Rusty v0 grammar

The grammar below is normative. `NEWLINE` and `;` are statement separators; one or more separators are required between adjacent statements unless a block boundary separates them. Whitespace and `//` comments are otherwise ignored.

```ebnf
program      = padding, function, { padding, function }, padding, EOF ;
function     = "fn", IDENT, "(", [ params ], ")", [ "->", type ], padding, block ;
params       = param, { ",", param } ;
param        = IDENT, ":", type ;
type         = "int" | "bool" | "string" ;
block        = "{", padding, [ statement, { separator, statement }, padding ], "}" ;
statement    = declaration | assignment | return | if | for | expression ;
declaration  = ( "let" | "var" ), IDENT, "=", expression ;
assignment   = IDENT, ( "=" | "+=" | "-=" ), expression ;
return       = "return", [ expression ] ;
if           = "if", expression, padding, block,
               [ padding, "else", padding, ( block | if ) ] ;
for          = "for", IDENT, "in", expression, "..", expression,
               padding, block ;
expression   = logical_or ;
logical_or   = logical_and, { "||", logical_and } ;
logical_and  = equality, { "&&", equality } ;
equality     = comparison, { ( "==" | "!=" ), comparison } ;
comparison   = term, { ( "<" | "<=" | ">" | ">=" ), term } ;
term         = factor, { ( "+" | "-" ), factor } ;
factor       = unary, { ( "*" | "/" | "%" ), unary } ;
unary        = ( "-" | "!" ), unary | primary ;
primary      = INT | STRING | "true" | "false" | IDENT
             | IDENT, "(", [ arguments ], ")"
             | "(", expression, ")" ;
arguments    = expression, { ",", expression } ;
separator    = ( NEWLINE | ";" ), padding ;
padding      = { NEWLINE | ";" } ;
```

Identifiers are ASCII letters or `_`, followed by ASCII letters, digits, or `_`. Integer literals are unsigned decimal text up to `INT64_MAX`; negative values use unary `-`. Strings are UTF-8 and support `\\n`, `\\r`, `\\t`, `\\"`, and `\\\\` escapes.