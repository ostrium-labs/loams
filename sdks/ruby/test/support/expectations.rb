# frozen_string_literal: true

require "loams"

module Loams
  module Test
    module Expectations
      STRUCTURAL_KEYS = %w[
        status reason grpcStatus identicalToStep frames frameKinds
        code message about httpStatus transport note
      ].freeze

      def self.check_error(fixture_name, step, expect, error)
        return [] unless expect.is_a?(Hash)

        problems = []

        if expect.key?("reason")
          want_reason = expect["reason"]
          if want_reason.nil?
            if error.reason
              problems << "#{fixture_name} step #{step}: expect.reason is null, and SDK reported #{error.reason}"
            end
          else
            got_reason = error.reason ? error.reason.to_s : error.unknown_reason
            if got_reason != want_reason
              problems << "#{fixture_name} step #{step}: expect.reason is #{want_reason}, and SDK reported #{got_reason || 'none'}"
            end
          end
        end

        if expect["grpcStatus"]
          want_code_num = expect["grpcStatus"].to_i
          want_code = GRPC_TO_CODE[want_code_num]
          if error.code != want_code
            problems << "#{fixture_name} step #{step}: expect.grpcStatus is #{want_code_num}, and SDK reported #{error.code}"
          end
        end

        problems
      end

      def self.check_success(fixture_name, step, status, expect, response, expected = true)
        problems = []
        unless expected
          problems << "#{fixture_name} step #{step}: recording answers #{status} but SDK returned a message"
          return problems
        end

        if response.nil?
          problems << "#{fixture_name} step #{step}: recording answers #{status} but SDK returned nothing"
          return problems
        end

        problems.concat(check_message(fixture_name, step, expect, response))
        problems
      end

      def self.check_message(fixture_name, step, expect, message)
        return [] unless expect.is_a?(Hash)

        problems = []
        expect.each do |key, want|
          next if STRUCTURAL_KEYS.include?(key)

          got = find_value(message, key)
          if got.nil?
            problems << "#{fixture_name} step #{step}: expect.#{key} is #{want.inspect}, but field was not found or nil"
            next
          end

          # Normalize for comparison
          if !compare_values(got, want)
            problems << "#{fixture_name} step #{step}: expect.#{key} is #{want.inspect}, but got #{got.inspect}"
          end
        end

        problems
      end

      def self.find_value(message, name)
        return nil if message.nil?

        raw_msg = message
        if message.respond_to?(:to_h) && !message.is_a?(Hash)
          message = message.to_h
        end

        snake_name = Descriptors.snake(name).to_sym
        sym_name = name.to_sym
        str_name = name.to_s
        snake_str = Descriptors.snake(name)

        if message.is_a?(Hash)
          [snake_name, sym_name, str_name, snake_str].each do |k|
            return message[k] if message.key?(k) && !message[k].nil?
          end

          # Check nested hashes
          message.each_value do |v|
            if v.is_a?(Hash)
              res = find_value(v, name)
              return res unless res.nil?
            end
          end
        end

        # If omitted from to_h because it holds the proto3 default value
        if raw_msg.class.respond_to?(:descriptor)
          desc = raw_msg.class.descriptor
          f = desc.lookup(snake_str) || desc.lookup(str_name) || desc.find { |field| field.json_name == str_name }
          if f
            case f.type
            when :bool then return false
            when :string then return ""
            when :int32, :int64, :uint32, :uint64, :sint32, :sint64 then return 0
            end
          end
        end

        nil
      end

      def self.compare_values(got, want)
        got = got.to_a if got.respond_to?(:to_a) && !got.is_a?(Hash)

        # Boolean
        if (got == true || got == false) || (want == true || want == false)
          return got == want
        end

        # Array containment or match
        if got.is_a?(Array) && want.is_a?(Array)
          return got == want || (want - got).empty?
        end
        return false if got.is_a?(Array) || want.is_a?(Array)

        return true if got == want

        # If got is an enum (Symbol or Integer) and want is String
        if got.is_a?(Symbol) || got.is_a?(String)
          return got.to_s.downcase == want.to_s.downcase ||
                 got.to_s.split("_").last&.downcase == want.to_s.downcase
        end

        # Numeric compare (int vs string in proto3 json)
        if got.is_a?(Numeric) && (want.is_a?(Numeric) || want =~ /^-?\d+$/)
          return got.to_i == want.to_i
        end

        got.to_s == want.to_s
      end
    end
  end
end
