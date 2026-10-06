# frozen_string_literal: true

$LOAD_PATH.unshift File.expand_path("../lib", __dir__)
$LOAD_PATH.unshift File.expand_path("support", __dir__)

require "minitest/autorun"
require "loams"
require "stub_transport"
require "expectations"
require "fixture_server"
require "corpus_driver"
