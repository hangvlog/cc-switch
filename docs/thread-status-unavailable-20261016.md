# Isolate stale entries in status batches

Desktop 3.19.72 keeps status refresh usable when a cached mobile row has been deleted or archived. A valid UUID that is absent from the eligible catalog returns only ID, unavailable status, source and observation time; no archived metadata is enriched. Invalid UUIDs and index failures still reject the request.

Reproduced against the live relay with one eligible ID and one missing ID: the previous version rejected the entire batch with 原任务不存在. The source probe returns both the real observation and a metadata-free unavailable row. The mobile sidebar removes unavailable rows before grouping.

This does not claim exact parity with an ordinarily launched official activity window: retained renderer membership remains unavailable without its snapshot channel.
