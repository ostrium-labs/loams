# The descriptor set, and every binding derived from it.
#
# Design §44 §7.3 has `protoc-gen-loams-facade` render one facade
# per language from the `loams.options.v1.module` and
# `loams.options.v1.facade` annotations, so thirteen SDKs cannot
# drift. This SDK has no generated code: it reads the **committed**
# `FileDescriptorSet` in `gen/` and derives every binding, module
# name and message type from it, which is the same property by a
# different route (D652) — a proto that adds an annotation changes
# the surface in the next build with nobody editing this file.
#
# ## Where the annotations come from
#
# `buf build` writes the options as **extension fields** on the
# `ServiceOptions` and `MethodOptions` messages, and the descriptor
# set carries them as unknown fields. `Google::Protobuf` parses the
# options into messages that keep those bytes, so `#to_proto` on an
# options message returns them, and `Loams::Wire` reads field 50001
# (the `module` annotation) and 50002 (the `facade` annotation) out
# of the raw bytes. The annotations are read from the same bytes
# protoc wrote, through the same descriptors, with no generated code
# and no registry to keep in step.
#
# ## One load, read once
#
# The set is parsed once and the resulting descriptors are immutable,
# so the tables are constants rather than something rebuilt per call.
require "google/protobuf"
require "google/protobuf/descriptor_pb"
require "loams/wire"

module Loams
  module Descriptors
    # The resource the descriptor set is read from, copied into the
    # gem by the build.
    DESCRIPTOR_SET = File.expand_path("../../gen/loams-descriptor.binpb", __dir__)

    # The proto revision this SDK was generated from (design §44 §10.3).
    PROTO_REV = "v1"

    # The field number the `module` extension occupies on `ServiceOptions`.
    MODULE_EXTENSION = 50001

    # The field number the `facade` extension occupies on `MethodOptions`.
    FACADE_EXTENSION = 50002

    # The parsed set, as a `Google::Protobuf::FileDescriptorSet`.
    SET = Google::Protobuf::FileDescriptorSet.decode(File.read(DESCRIPTOR_SET))

    # The pool every message is looked up in. Built in dependency
    # order: `add_serialized_file` links a file against the files
    # already in the pool, so a file whose imports are not yet added
    # is retried on a later pass, and a pass that makes no progress
    # is a missing import rather than an ordering accident.
    POOL = begin
      pool = Google::Protobuf::DescriptorPool.new
      pending = SET.file.to_a
      until pending.empty?
        progressed = false
        pending.reject! do |file|
          ready = file.dependency.all? { |dep| pool.lookup(dep) rescue false }
          if ready
            pool.add_serialized_file(file.to_proto)
            progressed = true
            true
          else
            false
          end
        end
        raise "the descriptor set has files whose imports are absent: #{pending.map(&:name).join(", ")}" unless progressed
      end
      pool
    end

    # Recursively collect every message in a file into MESSAGES,
    # keyed by fully qualified name.
    def self.collect_messages(messages, package, prefix)
      messages.each do |msg|
        full = prefix.empty? ? "#{package}.#{msg.name}" : "#{prefix}.#{msg.name}"
        MESSAGES[full] = POOL.lookup(full)
        collect_messages(msg.nested_type.to_a, package, full)
      end
    end

    # Every message in the set, keyed by its fully qualified proto name.
    MESSAGES = {}
    SET.file.each do |file|
      collect_messages(file.message_type, file.package, "")
    end

    # Every service in the set, in the order the files declare them.
    SERVICES = SET.file.flat_map do |file|
      file.service.to_a.map do |raw_svc|
        fqdn = "#{file.package}.#{raw_svc.name}"
        POOL.lookup(fqdn)
      end
    end

    # Every service in the set, keyed by its fully qualified name.
    SERVICES_BY_NAME = SERVICES.to_h do |svc|
      [svc.name, svc]
    end

    # One RPC, as the runtime dispatches it: everything a module
    # method hands the invoker and nothing the invoker has to guess.
    #
    # Every field is derived from the descriptor set.
    CallBinding = Struct.new(
      :rpc, :service, :method, :package_name, :streaming, :idempotency,
      :retry, :request, :response, :module, :facade_name, :summary,
      :unstable, :pagination,
      keyword_init: true,
    ) do
      # Whether the call answers with one message or with a stream.
      def server_streaming?
        streaming == :server
      end

      # Whether the request's **schema** declares a string
      # `idempotency_key`, which is what makes a mutation keyed (D610).
      def takes_idempotency_key?
        field = request.lookup("idempotency_key")
        !field.nil? && field.type == :string
      end

      # The two fields a paged call pages on, as
      # `FacadeOptions.pagination` names them (`"<items>:<next_page_token>"`),
      # or nil.
      def pagination_fields
        spec = pagination
        return nil if spec.nil? || spec.empty?
        parts = spec.split(":")
        return nil unless parts.size == 2
        items = response.lookup(parts[0]) || response.find { |f| f.json_name == parts[0] }
        token = response.lookup(parts[1]) || response.find { |f| f.json_name == parts[1] }
        return nil if items.nil? || token.nil?
        [items, token]
      end
    end

    # One module and the calls it exposes, both derived from the
    # descriptor set.
    ModuleBinding = Struct.new(
      :name, :summary, :package_name, :service, :unstable, :calls,
      keyword_init: true,
    )

    # `camelCase` or `PascalCase` to `snake_case`, which is what the
    # facade's method names are.
    def self.snake(name)
      out = +""
      name.each_char.with_index do |c, i|
        if c.match?(/[A-Z]/)
          out << "_" if i.positive? && (out[-1] != "_")
          out << c.downcase
        else
          out << c
        end
      end
      out
    end

    # The `loams.options.v1.ModuleOptions` on a service, read out of
    # the raw `ServiceOptions` bytes: `{name, summary, unstable}`.
    def self.parse_module_options(raw)
      payload = Wire.find_bytes(raw, MODULE_EXTENSION)
      return {} if payload.nil?
      {
        name: Wire.find_bytes(payload, 1),
        summary: Wire.find_bytes(payload, 2),
        unstable: !Wire.find_varint(payload, 3).nil? && Wire.find_varint(payload, 3) != 0,
      }
    end

    # The `loams.options.v1.FacadeOptions` on a method, read out of
    # the raw `MethodOptions` bytes: `{name, module, retry_safe, pagination}`.
    def self.parse_facade_options(raw)
      payload = Wire.find_bytes(raw, FACADE_EXTENSION)
      return {} if payload.nil?
      {
        module: Wire.find_bytes(payload, 1),
        name: Wire.find_bytes(payload, 2),
        retry_safe: !Wire.find_varint(payload, 3).nil? && Wire.find_varint(payload, 3) != 0,
        pagination: Wire.find_bytes(payload, 4),
      }
    end

    # Builds one binding from a service and a method, deciding every
    # field from the descriptor rather than from a table.
    def self.binding_for(svc, method)
      fd = svc.file_descriptor
      package = fd.to_proto.package
      service_full = svc.name

      svc_opts = svc.to_proto.options
      module_opts = svc_opts ? parse_module_options(svc_opts.to_proto) : {}

      method_opts = method.to_proto.options
      facade_opts = method_opts ? parse_facade_options(method_opts.to_proto) : {}

      idempotency = if method_opts
        case method_opts.idempotency_level
        when :IDEMPOTENT then :idempotent
        when :NO_SIDE_EFFECTS then :no_side_effects
        else :none
        end
      else
        :none
      end

      retry_class = if facade_opts[:retry_safe]
        :safe
      elsif idempotency == :none
        :manual
      else
        :safe
      end

      mod = facade_opts[:module]
      mod = mod.nil? || mod.empty? ? module_opts[:name] : mod
      facade_name = facade_opts[:name]
      facade_name = facade_name.nil? || facade_name.empty? ? snake(method.name) : facade_name

      request = method.input_type
      response = method.output_type

      CallBinding.new(
        rpc: "#{service_full}/#{method.name}",
        service: service_full,
        method: method.name,
        package_name: package,
        streaming: method.server_streaming ? :server : :unary,
        idempotency: idempotency,
        retry: retry_class,
        request: request,
        response: response,
        module: mod,
        facade_name: facade_name,
        summary: module_opts[:summary],
        unstable: module_opts[:unstable],
        pagination: facade_opts[:pagination],
      )
    end

    # Every RPC in the set, keyed by `<package>.Service/Method`.
    BINDINGS_BY_RPC = begin
      bindings = {}
      SERVICES.each do |svc|
        full = svc.name
        svc.each do |m|
          bindings["#{full}/#{m.name}"] = binding_for(svc, m)
        end
      end
      bindings
    end

    # The binding an RPC path names, or nil when no service in the set
    # declares it.
    def self.binding_for_rpc(rpc)
      BINDINGS_BY_RPC[rpc]
    end

    # The message a fully qualified proto name names, or nil.
    def self.message(full_name)
      MESSAGES[full_name]
    end

    # The service a fully qualified name names, or nil.
    def self.service(full_name)
      SERVICES_BY_NAME[full_name]
    end

    # The binding whose request or response type is +full_name+, or nil.
    def self.binding_for_message(full_name)
      BINDINGS_BY_RPC.values.find do |b|
        b.request.name == full_name || b.response.name == full_name
      end
    end

    # Every proto package in the set, as `GetInstance` names them.
    def self.proto_packages
      SET.file.map(&:package).select { |p| p.start_with?("loams.") }.uniq.sort
    end

    # The module catalogue, derived from the descriptor set and nothing else.
    def self.modules
      by_name = {}
      BINDINGS_BY_RPC.values.each do |binding|
        mod = binding.module
        next if mod.nil? || mod.empty?
        existing = by_name[mod]
        if existing.nil?
          by_name[mod] = ModuleBinding.new(
            name: mod,
            summary: binding.summary || "",
            package_name: binding.package_name,
            service: binding.service,
            unstable: binding.unstable,
            calls: [binding],
          )
        else
          existing.calls << binding
        end
      end
      by_name.values.map { |m| m.calls.sort_by!(&:facade_name); m }.sort_by(&:name)
    end

    # The module a name identifies, or nil.
    def self.find_module(name)
      modules.find { |m| m.name == name }
    end

    # The binding a module and a call name identify, or nil.
    def self.find_binding(module_name, call_name)
      mod = find_module(module_name)
      return nil if mod.nil?
      mod.calls.find { |c| c.facade_name == call_name }
    end
  end
end
