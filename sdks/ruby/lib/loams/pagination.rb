# frozen_string_literal: true

module Loams
  class PageIterator
    include Enumerable

    attr_reader :items_field, :next_page_token_field, :requested_tokens, :offered_tokens, :stopped_on_repeated_token

    def initialize(fetch:, items_field:, next_page_token_field:)
      @fetch = fetch
      @items_field = items_field
      @next_page_token_field = next_page_token_field
      @requested_tokens = []
      @offered_tokens = []
      @stopped_on_repeated_token = false

      validate_fields!
    end

    def each
      return to_enum(:each) unless block_given?

      started = false
      next_token = ""
      seen_tokens = []

      loop do
        token = started ? next_token : ""
        if started && (token.nil? || token.empty?)
          break
        end

        if started && seen_tokens.include?(token)
          @stopped_on_repeated_token = true
          break
        end

        @requested_tokens << token
        seen_tokens << token if started
        @offered_tokens << token if started

        page = @fetch.call(token)
        started = true

        # Extract items
        items = extract_field(page, @items_field) || []
        items.each { |item| yield item }

        # Extract next_page_token
        next_token = extract_field(page, @next_page_token_field).to_s
      end
    end

    private

    def validate_fields!
      # Validate items_field and next_page_token_field
      if @items_field.respond_to?(:label) && @items_field.label != :repeated
        raise ArgumentError, "#{@items_field.name} is not a repeated field"
      end

      if @items_field.respond_to?(:containing_type) && @next_page_token_field.respond_to?(:containing_type)
        if @items_field.containing_type != @next_page_token_field.containing_type
          raise ArgumentError, "fields declared on different messages"
        end
      end
    end

    def extract_field(message, field)
      return nil if message.nil?

      name = field.respond_to?(:name) ? field.name : field.to_s
      json_name = field.respond_to?(:json_name) ? field.json_name : nil

      if message.respond_to?(name)
        message.public_send(name)
      elsif json_name && message.respond_to?(json_name)
        message.public_send(json_name)
      elsif message.is_a?(Hash)
        message[name] || (json_name ? message[json_name] : nil) || message[name.to_sym]
      end
    end
  end
end
