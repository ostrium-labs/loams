// Conveniences on the generated `DocumentId` (a partial class): a document id is
// an unsigned 64-bit integer, a string or a UUID, and a caller should be able to
// write `1`, `"k-str"` or `DocumentId.OfUuid(…)` where one is expected.

using System.Globalization;

namespace Loams.Collection.V1;

public sealed partial class DocumentId
{
    /// <summary>A numeric id.</summary>
    public static DocumentId Of(ulong id) => new() { Uint = id };

    /// <summary>A string id.</summary>
    public static DocumentId Of(string id) => new() { String = id ?? throw new ArgumentNullException(nameof(id)) };

    /// <summary>A UUID id.</summary>
    public static DocumentId OfUuid(Guid id) => new() { Uuid = id.ToString() };

    /// <summary>A UUID id, as text.</summary>
    public static DocumentId OfUuid(string id) => new() { Uuid = id ?? throw new ArgumentNullException(nameof(id)) };

    public static implicit operator DocumentId(ulong id) => Of(id);

    public static implicit operator DocumentId(long id) =>
        id >= 0 ? Of((ulong)id) : throw new ArgumentOutOfRangeException(nameof(id), "a numeric document id is unsigned");

    public static implicit operator DocumentId(int id) => (long)id;

    public static implicit operator DocumentId(string id) => Of(id);

    public static implicit operator DocumentId(Guid id) => OfUuid(id);

    /// <summary>The id as text: the number, the string, or the UUID.</summary>
    public string Text => IdCase switch
    {
        IdOneofCase.Uint => Uint.ToString(CultureInfo.InvariantCulture),
        IdOneofCase.String => String,
        IdOneofCase.Uuid => Uuid,
        _ => string.Empty,
    };
}
