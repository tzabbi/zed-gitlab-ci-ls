; Identifiers

[
    (field)
    (field_identifier)
] @property

(variable) @variable

; Function calls

(function_call
  function: (identifier) @function)

(method_call
  method: (selector_expression
    field: (field_identifier) @function))

; Delimiters

"." @punctuation.delimiter
"," @punctuation.delimiter

"{{" @punctuation.bracket
"}}" @punctuation.bracket
"{{-" @punctuation.bracket
"-}}" @punctuation.bracket
")" @punctuation.bracket
"(" @punctuation.bracket

; Keywords

"default" @keyword
"include" @keyword
"stages" @keyword
"workflow" @keyword
"spec" @keyword
"after_script" @keyword
"allow_failure" @keyword
"before_script" @keyword
"cache" @keyword
"coverage" @keyword
"dast_configuration" @keyword
"dependencies" @keyword
"environment" @keyword
"extends" @keyword
"identity" @keyword
"image" @keyword
"inherit" @keyword
"interruptible" @keyword
"manual_confirmation" @keyword
"needs" @keyword
"pages" @keyword
"parallel" @keyword
"release" @keyword
"resource_group" @keyword
"retry" @keyword
"rules" @keyword
"script" @keyword
"run" @keyword
"secrets" @keyword
"services" @keyword
"stage" @keyword
"tags" @keyword
"timeout" @keyword
"trigger" @keyword
"when" @keyword

; Literals

[
  (interpreted_string_literal)
  (raw_string_literal)
  (rune_literal)
] @string

(escape_sequence) @string.special

[
  (int_literal)
  (float_literal)
  (imaginary_literal)
] @number

[
  (true)
  (false)
  (nil)
] @constant.builtin

(comment) @comment
(ERROR) @error
