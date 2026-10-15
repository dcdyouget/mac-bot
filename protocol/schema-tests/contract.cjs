const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const Ajv = require('ajv');
// schemars emits uint32/uint64 formats, which are not standard Ajv formats.
// Their numeric type and bounds are still validated.
const ajv = new Ajv({format: false});
const read = file => JSON.parse(fs.readFileSync(file, 'utf8'));
const schemaRoot = process.env.MACBOT_SCHEMA_TEST_DIR || 'protocol/schema';
const schema = name => read(path.join(schemaRoot, `${name}.json`));
const event = ajv.compile(schema('event_frame'));
const request = ajv.compile(schema('rpc_request'));
const trace = ajv.compile(schema('trace_item'));

for (const [file, title, discriminator, enumName] of [
    ['rpc_request', 'RpcRequest', 'method', 'Method'],
    ['event_frame', 'EventFrame', 'event', 'EventName'],
    ['trace_item', 'TraceItem', 'type', 'TraceType'],
]) {
    const document = schema(file);
    const bound = document.allOf?.map(branch => branch.if.properties[discriminator].const) || [];
    assert.deepEqual([...bound].sort(), [...document.definitions[enumName].enum].sort(), `${title}: every known discriminator must have exactly one binding`);
}

const valid = (validate, value, source) => assert.equal(validate(value), true, `${source}: ${JSON.stringify(validate.errors)}`);
for (const file of fs.readdirSync('protocol/fixtures/events')) {
    const value = read(`protocol/fixtures/events/${file}`);
    valid(event, value, file);
    valid(event, {...value, future_envelope: {kept: true}, data: {...value.data, future_payload: 42}}, `${file} extensions`);
}
for (const file of fs.readdirSync('protocol/fixtures/trace')) {
    const value = read(`protocol/fixtures/trace/${file}`);
    valid(trace, value, file);
    valid(trace, {...value, future_envelope: true, data: {...value.data, future_payload: {kept: true}}}, `${file} extensions`);
}
valid(request, {v: 1, kind: 'req', id: 'r', method: 'chat.send', params: {chat_id: 'chat_1', text: 'hello', mentions: [], future_payload: true}, future_envelope: true}, 'chat.send extensions');
valid(request, {v: 1, kind: 'req', id: 'r', method: 'ping', params: {future_payload: true}}, 'ping extensions');
assert.equal(request({v: 1, kind: 'req', id: 'r', method: 'chat.send', params: {}}), false, 'chat.send must not decode as EmptyParams');
const typing = read('protocol/fixtures/events/typing.json');
assert.equal(event({...typing, event: 'message.deleted'}), false, 'message.deleted must not decode typing data');
const start = read('protocol/fixtures/trace/run_start.json');
assert.equal(trace({...start, type: 'tool.end'}), false, 'tool.end must not decode run.start data');
const traceEvent = read('protocol/fixtures/events/trace_item.json');
assert.equal(event({...traceEvent, data: {...traceEvent.data, item: {...start, type: 'tool.end'}}}), false, 'nested TraceItem must have discriminator binding');
// Unknown enum names remain the existing schema boundary. Clients independently
// ignore unknown events and render unknown blocks through their raw compatibility layer.
console.log('Schema discriminator bindings, fixtures, nested traces and extension fields passed.');
