# #217 — Name a gRPC call by the path it arrived on

Merged 2026-08-31 into `master` from `fix/grpc-method-path`, commit [`e9c3701`](https://github.com/ulo-rs/ulo/commit/e9c370176fe0ed54f9d6f3918809c7aff0606be8).

`GrpcContext::method()` returned `Orders/create`: the trait's last path segment and the Rust method name, which is what an impl block shows. Its rustdoc promised the wire path, `orders.OrdersService/CreateOrder`, and the reference documentation repeated it.

Neither the package nor the route's casing is derivable from an impl block, and on a service built through `tonic_build::manual` the route name is an independent field that need not resemble the Rust one. So a guard or interceptor matching on the method path was matching a string that appears in no proto file, no log line and no client stub, and an operator who copied `toni_test.orders.Orders/Create` out of the proto never matched.

## Where the path comes from

The request. `MethodPathLayer` reads `req.uri().path()` and puts it on the request as `GrpcMethodPath`; the generated method prefers it and falls back to the names the macro can see, for a pipeline driven without an adapter. tonic inserts its own `GrpcMethod` extension in generated *client* code only, so the server side has nothing to read.

The layer annotates the request and returns the inner future untouched, so it adds no allocation per call.

## What changes for a reader of `ctx.method()`

```
before   Orders/create
after    toni_test.orders.Orders/Create
```

Two tests moved with it: the one pinning what a handler reads off the request, and the metadata-overlay test that keys its record on the method path.
