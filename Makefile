# The RPC conformance suite against a live Kafka. Minutes per run (578 s and
# 147 s on two runs, against 8-30 s for each of the other brokers), so CI leaves
# it to this target; nats, redis, mqtt and rabbitmq run in CI's
# `rpc conformance (brokers)` job on every push to master. Needs Docker.
.PHONY: conformance-kafka
conformance-kafka:
	cargo test -p ulo-rpc-kafka --features integration --test conformance --locked
