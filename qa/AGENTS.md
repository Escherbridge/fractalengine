# QA evidence storage

Focused pass packets contain hashes of actual capture bytes. Add a directory-specific
`-text` rule in the root `.gitattributes` before staging a new pass packet. Preserve
those bytes through Git; do not normalize line endings after computing its manifest.

Keep original application captures immutable. Derived reports may be revised after
independent review, with a regenerated manifest and an explicit anchor to the reviewed
manifest. Check both working-file and staged Git-blob bytes before committing.

Legacy pass directories keep their existing Git attributes. Do not renormalize them
without checking their original retained capture bytes and reviewed manifests.
