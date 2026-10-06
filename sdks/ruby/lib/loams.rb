# frozen_string_literal: true

require "loams/version"
require "loams/wire"
require "loams/descriptors"
require "loams/envelopes"
require "loams/codec"
require "loams/errors"
require "loams/idempotency"
require "loams/token_source"
require "loams/retry"
require "loams/transport"
require "loams/streams"
require "loams/invoker"
require "loams/pagination"
require "loams/client"

module Loams
  def self.client(endpoint = "http://127.0.0.1:4400", **options)
    Client.new(endpoint, **options)
  end
end
