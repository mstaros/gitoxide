namespace GixSharp.Native;

// Interoptopus emits SliceMutByte as partial. Reusing one unowned view avoids a
// managed wrapper allocation per pull while the Stream lock prevents overlap.
public partial class SliceMutByte
{
    internal static SliceMutByte CreateUnownedForByteStreamingPrototype() => new();

    internal void PointAtUnownedForByteStreamingPrototype(IntPtr data, ulong length)
    {
        if (_handle.IsAllocated)
            throw new InvalidOperationException("The prototype view must not own a managed pin.");

        _data = data;
        _len = length;
    }

    internal void InvalidateUnownedForByteStreamingPrototype()
    {
        _data = IntPtr.Zero;
        _len = 0;
    }
}
