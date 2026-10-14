# Spelling word list

`en-US.words` is the word list the spelling check (`crates/engine/src/spell.rs`) reads. It is
the American English list of SCOWL (Spell Checker Oriented Word Lists, by Kevin Atkinson), size
50, as Debian and Ubuntu ship it in the `wamerican` package (SCOWL 2020.12.07; the SCOWL lists
it is made of are named in that package's `wamerican.scowl-word-lists-used`).

Modified from the original: possessives (`…'s`) were left out, every word was lower-cased and
duplicates removed, and the sorted list is written with front coding (each line is the number of
letters it shares with the line before, 0–9, then the rest of the word) to make it smaller.

The licence and the notices of the lists SCOWL is made from are in `SCOWL-LICENSE.txt`, as they
come with the package; they allow use, modification and redistribution, with the notices kept.
