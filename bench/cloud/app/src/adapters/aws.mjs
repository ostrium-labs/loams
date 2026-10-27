// AWS ports: items on DynamoDB, queue on SQS. AWS SDK v3, which the Lambda Node
// runtime provides (the bundle marks it external). maxAttempts 1: a throttle is
// reported as 429, not hidden behind SDK retries (CB1 plan §4).
import { DynamoDBClient, GetItemCommand, PutItemCommand, QueryCommand } from "@aws-sdk/client-dynamodb";
import { SQSClient, SendMessageCommand } from "@aws-sdk/client-sqs";
import { Throttled } from "../app.mjs";

const THROTTLES = new Set([
  "ProvisionedThroughputExceededException",
  "ThrottlingException",
  "RequestLimitExceeded",
  "TooManyRequestsException",
]);

function wrap(err) {
  if (THROTTLES.has(err?.name)) return new Throttled(err.name);
  return err;
}

export function awsEnv(environ = process.env) {
  const region = environ.AWS_REGION;
  const ddb = new DynamoDBClient({ region, maxAttempts: 1 });
  const sqs = new SQSClient({ region, maxAttempts: 1 });
  const table = environ.LOAMS_ITEMS;
  const queueUrl = environ.LOAMS_QUEUE_URL;

  return {
    target: "aws",
    items: {
      async get({ pk, sk }, { consistent = true } = {}) {
        try {
          const out = await ddb.send(
            new GetItemCommand({ TableName: table, Key: { pk: { S: pk }, sk: { S: sk } }, ConsistentRead: consistent }),
          );
          if (!out.Item) return null;
          return { key: { pk, sk }, value: JSON.parse(out.Item.v.S), version: out.Item.ver.S };
        } catch (err) {
          throw wrap(err);
        }
      },
      async put({ pk, sk }, value) {
        const version = `${Date.now()}-${Math.random().toString(16).slice(2, 10)}`;
        try {
          await ddb.send(
            new PutItemCommand({
              TableName: table,
              Item: { pk: { S: pk }, sk: { S: sk }, v: { S: JSON.stringify(value) }, ver: { S: version } },
            }),
          );
          return { version };
        } catch (err) {
          throw wrap(err);
        }
      },
      async list(pk, { after, limit = 100 } = {}) {
        try {
          const out = await ddb.send(
            new QueryCommand({
              TableName: table,
              KeyConditionExpression: "pk = :pk",
              ExpressionAttributeValues: { ":pk": { S: pk } },
              ExclusiveStartKey: after ? { pk: { S: pk }, sk: { S: after } } : undefined,
              Limit: limit,
              ConsistentRead: true,
            }),
          );
          const items = (out.Items ?? []).map((it) => ({
            key: { pk, sk: it.sk.S },
            value: JSON.parse(it.v.S),
            version: it.ver.S,
          }));
          return { items, next: out.LastEvaluatedKey ? out.LastEvaluatedKey.sk.S : undefined };
        } catch (err) {
          throw wrap(err);
        }
      },
    },
    queue: {
      async send(_queue, event) {
        try {
          await sqs.send(new SendMessageCommand({ QueueUrl: queueUrl, MessageBody: JSON.stringify(event) }));
        } catch (err) {
          throw wrap(err);
        }
      },
    },
  };
}
