// Liquid glass over the video: the window's floating elements and its four curtains are one signed
// distance field. Inside it the picture is refracted by a thick lens rim (a drop's profile), split
// into colours at the rim, and lit by two highlights. Elements that come close merge like drops.
// All lengths are in pixels; PX is pixels per DIP.

cbuffer Scene : register(b0)
{
    float2 R;      // target size
    float PX;
    float S;       // lens strength, 0..2
    float D;       // dispersion, 0..2
    float TH;      // theme: 0 graphite, 1 frost
    float TI;      // curtain tint, 0..1
    float N;       // number of elements
    float4 VR;     // video rectangle: x, y, w, h (w = 0: no video)
    float4 BG;     // colour around the video
    float4 EM;     // each curtain's deepest point: left, right, top, bottom
    float4 B[12];  // elements: centre, half size
    float4 RC[12]; // corner radius, bevel cap
    float4 TC[12]; // tint: rgb, amount
    float4 E[52];  // curtain edges, one side per component: B-spline control points
};

Texture2D<float4> Video : register(t0);
SamplerState Linear : register(s0);

float4 vs(uint id : SV_VertexID) : SV_Position
{
    // One triangle over the whole target.
    float2 p = float2((id << 1) & 2, id & 2);
    return float4(p * float2(2, -2) + float2(-1, 1), 0, 1);
}

float3 tex(float2 p)
{
    float2 uv = (p - VR.xy) / max(VR.zw, 1);
    if (VR.z <= 0 || any(uv < 0) || any(uv > 1))
        return BG.rgb;
    return Video.SampleLevel(Linear, uv, 0).rgb;
}

float box(float2 p, float4 b, float r)
{
    float2 q = abs(p - b.xy) - b.zw + r;
    return length(max(q, 0)) + min(max(q.x, q.y), 0) - r;
}

// Distance to the curtains; ex is each side's edge at this point (how far it reaches in).
float curtains(float2 p, out float4 ex)
{
    float4 v = float4(p.x, R.x - p.x, p.y, R.y - p.y);
    float4 lo = v - EM;
    float m = min(min(lo.x, lo.y), min(lo.z, lo.w));
    ex = -1e5;
    if (m > 60 * PX)
        return m;
    // Quadratic B-spline through the edge's points: only the three nearest count.
    float4 f = float4(p.y / R.y, p.y / R.y, p.x / R.x, p.x / R.x) * 47 + 2;
    float4 c = floor(f + 0.5);
    float4 x = 0, dx = 0;
    [unroll] for (int k = -1; k <= 1; k++)
    {
        float4 i = clamp(c + k, 0, 51);
        float4 u = f - i, a = abs(u);
        float4 in1 = step(a, 0.5), in2 = step(a, 1.5);
        float4 w = lerp(in2 * 0.5 * (1.5 - a) * (1.5 - a), 0.75 - u * u, in1);
        float4 dw = lerp(-in2 * (1.5 - a) * sign(u), -2 * u, in1);
        float4 e = float4(E[(int)i.x].x, E[(int)i.y].y, E[(int)i.z].z, E[(int)i.w].w);
        x += e * w;
        dx += e * dw;
    }
    ex = x;
    dx *= 47 / float4(R.y, R.y, R.x, R.x);
    float4 d = (v - x) / sqrt(1 + dx * dx);
    d = lerp(d, lo, step(60 * PX, lo));
    return min(min(d.x, d.y), min(d.z, d.w));
}

// The whole glass. cap limits the rim to what the element can hold; mem is how much the point belongs
// to a curtain rather than to an element (blended softly where they meet); tint is the elements' own.
float sdf(float2 p, out float cap, out float mem, out float4 tint, out float4 ex)
{
    float d = curtains(p, ex);
    cap = 1e4;
    mem = 1;
    tint = 0;
    float k = 30 * PX, k2 = 70 * PX;
    int n = (int)N;
    [loop] for (int i = 0; i < n; i++)
    {
        float di = box(p, B[i], RC[i].x);
        float h = saturate(0.5 + 0.5 * (di - d) / k);
        mem *= smoothstep(0, 1, saturate(0.5 + 0.5 * (di - d) / k2));
        d = lerp(di, d, h) - k * h * (1 - h);
        cap = lerp(RC[i].y, cap, h);
        tint = lerp(TC[i], tint, h);
    }
    return d;
}

float field(float2 p)
{
    float c, m;
    float4 t, e;
    return sdf(p, c, m, t, e);
}

float4 ps(float4 pos : SV_Position) : SV_Target
{
    float2 p = pos.xy;
    float cap, mem;
    float4 tint, ex;
    float d = sdf(p, cap, mem, tint, ex);
    float3 bg = tex(p);
    // A soft shadow around the glass.
    float3 outside = bg * (1 - 0.14 * exp(-max(d, 0) / (14 * PX)));
    if (d > 2 * PX)
        return float4(outside, 1);

    float bev = min((12 + 18 * S) * PX, cap * 0.98);
    float e = max(PX, bev * 0.12);
    float2 g = float2(field(p + float2(e, 0)) - field(p - float2(e, 0)), field(p + float2(0, e)) - field(p - float2(0, e)));
    g /= max(length(g), 1e-4);
    // A drop's profile: flat in the middle, steep at the rim.
    float t = saturate(-d / bev);
    float x = 1 - t;
    float h = sqrt(max(1 - x * x, 0.0025));
    float slope = min(x / h, 5);
    float2 off = g * slope * (5 + 16 * S) * PX;

    // Dispersion: red, green and blue bend by different amounts.
    float spread = 0.12 + 0.32 * D;
    float3 c = 0, ws = 0;
    [unroll] for (int i = 0; i < 9; i++)
    {
        float f = i / 8.0;
        float3 w = max(0, 1 - abs(f - float3(0, 0.5, 1)) * 2);
        float kk = 1 + (f - 0.5) * spread * 2;
        c += tex(p + off * kk) * w;
        ws += w;
    }
    c /= ws;

    // The curtain's tint follows the refracted point, so it flows through the lens.
    float c0, memR;
    float4 t0, e0;
    sdf(p + off * 0.6, c0, memR, t0, e0);
    float l = dot(c, float3(0.299, 0.587, 0.114));
    c = lerp(l, c, 1 + x * x * (0.4 + 0.7 * D));
    c = TH < 0.5 ? c * 0.8 : lerp(c, 1, 0.12);
    c = lerp(c, tint.rgb, tint.a * (1 - memR));
    c = lerp(c, TH < 0.5 ? float3(0.07, 0.08, 0.095) : float3(0.94, 0.955, 0.97), TI * memR);

    float3 n = normalize(float3(g * x, h));
    float3 L = normalize(float3(-0.55, -0.75, 0.65)), H = normalize(L + float3(0, 0, 1));
    float spec = pow(max(dot(n, H), 0), 48) * (0.35 + 0.45 * S);
    float3 L2 = normalize(float3(0.55, 0.75, 0.65)), H2 = normalize(L2 + float3(0, 0, 1));
    spec += pow(max(dot(n, H2), 0), 70) * (0.15 + 0.25 * S);
    float fres = pow(1 - n.z, 3) * (TH < 0.5 ? 0.22 : 0.35);
    float rim = smoothstep(1.6 * PX, 0, abs(d + 1.1 * PX)) * (0.2 + 0.8 * abs(dot(g, normalize(float2(-0.6, -0.8)))));
    c += spec + fres + rim * 0.85;
    return float4(lerp(outside, c, smoothstep(PX, -PX, d)), 1);
}
