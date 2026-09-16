; The document -> node -> mapping -> pair structure also matches flow mappings.
; Tags are checked as syntax nodes, so anchors and comments cannot hide !reference.
; Zed injects whole scalars: quotes and block headers cannot be trimmed here.

; Global before_script and after_script.
((document
  (_
    (_
      (_
        key: (flow_node
          [(plain_scalar) (single_quote_scalar) (double_quote_scalar)] @_script)
        value: (_
          (tag)? @_tag
          [
            (plain_scalar (string_scalar) @injection.content)
            (single_quote_scalar) @injection.content
            (double_quote_scalar) @injection.content
            (block_scalar) @injection.content
            (block_sequence
              (block_sequence_item
                (_
                  (tag)? @_item_tag
                  [
                    (plain_scalar (string_scalar) @injection.content)
                    (single_quote_scalar) @injection.content
                    (double_quote_scalar) @injection.content
                    (block_scalar) @injection.content
                  ])))
            (flow_sequence
              (flow_node
                (tag)? @_item_tag
                [
                  (plain_scalar (string_scalar) @injection.content)
                  (single_quote_scalar) @injection.content
                  (double_quote_scalar) @injection.content
                ]))
          ])))))
  (#any-of? @_script
    "before_script" "after_script"
    "'before_script'" "'after_script'"
    "\"before_script\"" "\"after_script\"")
  (#not-eq? @_tag "!reference")
  (#not-eq? @_item_tag "!reference")
  (#set! injection.language "bash"))

; Direct job/default properties, excluding reserved root metadata as job names.
; "default" and the conventional "pages" job intentionally remain eligible.
((document
  (_
    (_
      (_
        key: (flow_node
          [(plain_scalar) (single_quote_scalar) (double_quote_scalar)] @_job)
        value: (_
          (_
            (_
              key: (flow_node
                [(plain_scalar) (single_quote_scalar) (double_quote_scalar)] @_script)
              value: (_
                (tag)? @_tag
                [
                  (plain_scalar (string_scalar) @injection.content)
                  (single_quote_scalar) @injection.content
                  (double_quote_scalar) @injection.content
                  (block_scalar) @injection.content
                  (block_sequence
                    (block_sequence_item
                      (_
                        (tag)? @_item_tag
                        [
                          (plain_scalar (string_scalar) @injection.content)
                          (single_quote_scalar) @injection.content
                          (double_quote_scalar) @injection.content
                          (block_scalar) @injection.content
                        ])))
                  (flow_sequence
                    (flow_node
                      (tag)? @_item_tag
                      [
                        (plain_scalar (string_scalar) @injection.content)
                        (single_quote_scalar) @injection.content
                        (double_quote_scalar) @injection.content
                      ]))
                ]))))))))
  (#not-match? @_job "^[\"']?(after_script|before_script|cache|hooks|image|include|nil|pre_get_sources_script|script|services|spec|stages|true|false|types|variables|workflow)[\"']?$")
  (#any-of? @_script
    "script" "before_script" "after_script" "pre_get_sources_script"
    "'script'" "'before_script'" "'after_script'" "'pre_get_sources_script'"
    "\"script\"" "\"before_script\"" "\"after_script\"" "\"pre_get_sources_script\"")
  (#not-eq? @_tag "!reference")
  (#not-eq? @_item_tag "!reference")
  (#set! injection.language "bash"))

; Only a direct hooks property of a job/default can introduce hook commands.
((document
  (_
    (_
      (_
        key: (flow_node
          [(plain_scalar) (single_quote_scalar) (double_quote_scalar)] @_job)
        value: (_
          (_
            (_
              key: (flow_node
                [(plain_scalar) (single_quote_scalar) (double_quote_scalar)] @_hooks)
              value: (_
                (_
                  (_
                    key: (flow_node
                      [(plain_scalar) (single_quote_scalar) (double_quote_scalar)] @_script)
                    value: (_
                      (tag)? @_tag
                      [
                        (plain_scalar (string_scalar) @injection.content)
                        (single_quote_scalar) @injection.content
                        (double_quote_scalar) @injection.content
                        (block_scalar) @injection.content
                        (block_sequence
                          (block_sequence_item
                            (_
                              (tag)? @_item_tag
                              [
                                (plain_scalar (string_scalar) @injection.content)
                                (single_quote_scalar) @injection.content
                                (double_quote_scalar) @injection.content
                                (block_scalar) @injection.content
                              ])))
                        (flow_sequence
                          (flow_node
                            (tag)? @_item_tag
                            [
                              (plain_scalar (string_scalar) @injection.content)
                              (single_quote_scalar) @injection.content
                              (double_quote_scalar) @injection.content
                            ]))
                      ])))))))))))
  (#not-match? @_job "^[\"']?(after_script|before_script|cache|hooks|image|include|nil|pre_get_sources_script|script|services|spec|stages|true|false|types|variables|workflow)[\"']?$")
  (#any-of? @_hooks "hooks" "'hooks'" "\"hooks\"")
  (#any-of? @_script "pre_get_sources_script" "'pre_get_sources_script'" "\"pre_get_sources_script\"")
  (#not-eq? @_tag "!reference")
  (#not-eq? @_item_tag "!reference")
  (#set! injection.language "bash"))
