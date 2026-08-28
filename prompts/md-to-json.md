---
name: md_to_json
description: Exercise the md_to_json Lua global on a small sample paper
promptforge: 1
---

# md_to_json

---

Chunks an embedded sample paper and returns a typed block summary. Pure Lua, no model.

## Main

```lua
local fence = string.rep("`", 3)
local md = table.concat({
  "Preamble paragraph.",
  "",
  "# 1 Introduction",
  "",
  "We propose a change to...",
  "",
  fence .. "cpp",
  "int x; // must be trivially relocatable",
  fence,
  "",
  "## 1.1 Motivation",
  "",
  "| A | B |",
  "|---|---|",
  "| 1 | 2 |",
  "",
  "- first",
  "- second",
  "",
  "> a quote",
  "",
  "---",
  "",
  "<div>",
  "html",
  "</div>",
}, "\n") .. "\n"

store.write("paper.md", md)
local b = md_to_json(store.read("paper.md"))

local function expect(cond, msg)
  if not cond then error(msg, 2) end
end

expect(#b == 10, "expected 10 blocks, got " .. tostring(#b))

expect(b[1].type == "paragraph", "1 type")
expect(b[1].content == "Preamble paragraph.\n", "1 content")
expect(b[1].line == 1, "1 line")
expect(b[1].section[1] == nil, "1 preamble section")
expect(b[1].lang == nil, "1 lang absent")

expect(b[2].type == "h1", "2 type")
expect(b[2].content == "1 Introduction", "2 content")
expect(b[2].line == 3, "2 line")
expect(b[2].section[1] == "1 Introduction", "2 section")
expect(b[2].section[2] == nil, "2 section length")

expect(b[3].type == "paragraph", "3 type")
expect(b[3].content == "We propose a change to...\n", "3 content")
expect(b[3].line == 5, "3 line")
expect(b[3].section[1] == "1 Introduction", "3 section")

expect(b[4].type == "code_block", "4 type")
expect(b[4].lang == "cpp", "4 lang")
expect(b[4].content == "int x; // must be trivially relocatable\n", "4 content")
expect(b[4].line == 7, "4 line")
expect(b[4].section[1] == "1 Introduction", "4 section")

expect(b[5].type == "h2", "5 type")
expect(b[5].content == "1.1 Motivation", "5 content")
expect(b[5].line == 11, "5 line")
expect(b[5].section[1] == "1 Introduction", "5 section[1]")
expect(b[5].section[2] == "1.1 Motivation", "5 section[2]")

expect(b[6].type == "table", "6 type")
expect(b[6].line == 13, "6 line")
expect(b[6].lang == nil, "6 lang absent")
expect(b[6].section[2] == "1.1 Motivation", "6 section")

expect(b[7].type == "list", "7 type")
expect(b[7].line == 17, "7 line")

expect(b[8].type == "blockquote", "8 type")
expect(b[8].line == 20, "8 line")

expect(b[9].type == "thematic_break", "9 type")
expect(b[9].content == "---\n", "9 content")
expect(b[9].line == 22, "9 line")

expect(b[10].type == "html_block", "10 type")
expect(b[10].content == "<div>\nhtml\n</div>\n", "10 content")
expect(b[10].line == 24, "10 line")
expect(b[10].section[2] == "1.1 Motivation", "10 still under h2")

local lines = { "ok " .. #b .. " blocks" }
for i, block in ipairs(b) do
  local row = i .. " " .. block.type .. " line=" .. block.line
  if block.lang ~= nil then
    row = row .. " lang=" .. block.lang
  end
  if #block.section > 0 then
    row = row .. " section=" .. table.concat(block.section, " > ")
  end
  table.insert(lines, row)
end
return table.concat(lines, "\n")
```
