# data_model

Schemas, types, validation rules. For each affected module: what data shapes are added or modified, how are they validated, are there serialization implications.

Changes to data_model in any engine module typically ripple into `persistence` (PG schema), `data_model` of consumers, and often `documentation` (spec/02-data-model.md).
