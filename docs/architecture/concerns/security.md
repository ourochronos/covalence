# security

Clearance levels (default `0` = local_strict, INV-7), secrets handling (no hardcoded credentials), file-system safety, egress filtering at query time, federation guards.

All data defaults to local-strict; promotion to federated clearance is explicit and triggers recursive recalculation of derived entities.
