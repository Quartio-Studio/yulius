# 0393. Sandbox port ranges are reused after retirement

## Status

Accepted.

## Context

The unique port base in each sandbox row kept removed sandboxes' ranges reserved indefinitely.
Removing a sandbox starts asynchronous retirement. Its holder and browser may still be using
the assigned ports when the removal request returns.

## Decision

Keep the original range on the sandbox record. A partial unique index reserves each base
until runtime checks establish that the removed sandbox has ended its work, its holder can no
longer start, and the host loopback ports are free. The allocator rechecks these conditions
when a new sandbox needs a range. It uses complete blocks of 20 between 20000 and 32767.

## Alternatives

Releasing at removal would overlap a running relay. Reserving every historical range
eventually enters Linux’s default ephemeral port range and exhausts the address space.

## Consequences

Upgrading preserves the historical rows and initially keeps every existing reservation.
A holder that is still retiring delays reuse. A full pool refuses creation until a block
becomes available. Existing sandboxes keep their port numbers.
