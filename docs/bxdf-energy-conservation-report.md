# BxDF energy conservation: research and Zenith implementation plan

Date: 2026-09-15. Code inspected: working tree based on `5c5bc78`, including existing uncommitted lighting, shadow, AO, and post-processing changes. This report proposes implementation; it does not change renderer behavior.

## Recommendation

Implement **Kulla–Conty specular multiple-scattering compensation, with the corrected Fresnel coefficient, plus a normalized diffuse energy budget**. Reuse Zenith's existing GGX integration and environment resources.

This is a good fit for Zenith's current deferred renderer: one directional light, isotropic metallic/roughness materials, and split-sum image-based lighting (IBL). It gives a reciprocal direct-light BRDF with explicit energy accounting. The principal costs are two additional small-table samples per shaded pixel and a small initialization pass. These are structural estimates, not GPU timings.

**Turquin/Filament scaling is the cheaper alternative** if profiling makes those samples unacceptable. It fits the existing two-channel LUT without extra storage, but sacrifices reciprocity. Full stochastic microfacet scattering belongs in a reference implementation or future path tracer.

## 1. What must be conserved?

BxDF covers reflection and transmission distributions; Zenith currently evaluates opaque BRDFs. For a non-emissive reflective surface, define the directional reflectance for a fixed incident direction:

\[
R(l)=\int_{\Omega^+} f(l,v)(n\cdot v)\,d\omega_v.
\]

- **Energy conservation:** `0 <= R(l) <= 1`. Absorption is allowed.
- **Energy preservation:** `R(l) = 1` for a lossless opaque material.
- **Reciprocity:** `f(l,v) = f(v,l)` in the same medium.

A white furnace evaluates the other integral, over incident directions at fixed view. Reciprocity makes the two equivalent. A non-reciprocal model can pass the white furnace without satisfying the forward energy constraint. These properties must be tested separately. [OpenPBR definitions](https://academysoftwarefoundation.github.io/OpenPBR/)

A physically normalized single-scattering GGX lobe usually loses energy rather than creating it: masking removes paths that would bounce again between microfacets. Recovering those paths is a preservation problem. Separately, combining specular and diffuse without sharing their energy budget can create excess reflection. Scene GI, AO, and shadow visibility do not replace this local scattering calculation.

For transmissive materials, test reflected plus transmitted **power**, including the BSDF's transport convention and refractive-index factors. Do not require a glass reflection lobe alone, or an absorbing colored metal, to return one.

## 2. Relevant papers and practical methods

“Popular” here means established research or documented production approaches, not a citation-count ranking.

| Method / primary source | Pros | Cons | Fit for Zenith |
| --- | --- | --- | --- |
| Single-scattering GGX + Fresnel-weighted diffuse, the current baseline | Simple; normalized specular is passive; familiar material response. | Missing interreflection darkens rough metals. Pointwise Fresnel weighting does not establish a conserved sum of lobes. | Keep as a comparison mode during development. |
| **Kulla & Conty, 2017**, *Revisiting Physically Based Shading at Imageworks*, with **Hill's 2018 correction** | Reciprocal additive compensation; lossless furnace normalization; closed-form evaluation; handles colored absorption approximately. | Broad, azimuth-independent compensation differs from the real scattering distribution. Needs directional and average albedo data. | **Preferred direct-light BRDF.** [Course and slides](https://blog.selfshadow.com/publications/s2017-shading-course/), [corrected derivation](https://blog.selfshadow.com/2018/06/04/multi-faceted-part-2/) |
| **Turquin, 2019**, *Practical Multiple Scattering Compensation for Microfacet Models*; documented in Filament | Scales the existing lobe; very low integration cost; retains its sampling distribution; no extra energy table in Zenith. | View-dependent scaling is non-reciprocal. Angular shape and repeated Fresnel absorption are approximate; a passing furnace is insufficient validation. | Best minimal-cost alternative. [Technical report](https://blog.selfshadow.com/publications/turquin/ms_comp_final.pdf), [Filament](https://google.github.io/filament/main/filament.html) |
| **Fdez-Agüera, 2019**, *A Multiple-Scattering Microfacet Model for Real-Time Image Based Lighting* | Reuses ordinary split-sum coefficients and diffuse irradiance; low overhead; includes an explicit diffuse energy remainder. | Designed for environment lighting. Broad illumination approximation is weaker for narrow lights; does not supply a matching reciprocal direct-light BRDF. | Strong IBL alternative; compare against the same reference, rather than stacking it over another compensation. [JCGT paper](https://jcgt.org/published/0008/01/03/paper.pdf) |
| **Heitz, Hanika, d'Eon & Dachsbacher, 2016**, *Multiple-Scattering Microfacet BSDFs with the Smith Model* | Simulates the missing scattering within the Smith model; conservation and reciprocity in expectation; supports conductors, dielectrics, diffuse microfacets, and anisotropy. | Stochastic evaluation, variable work, and variance; unsuitable for Zenith's current deterministic shading pass. | Reference model and future offline/path-traced BxDFs. [Authors' project, paper, and implementation](https://eheitzresearch.wordpress.com/240-2/) |
| **Bitterli & d'Eon, 2022**, *A Position-Free Path Integral for Homogeneous Slabs and Multiple Scattering on Smith Microfacets* | Analytically removes collision-distance dimensions to reduce variance; unbiased conductor treatment. | Still stochastic and substantially more involved than a LUT correction; not a constant-cost replacement shader. | More recent reference/future path-tracing option. [Paper](https://arxiv.org/abs/2205.00587) |
| **Belcour, 2018**, *Efficient Rendering of Layered Materials using an Atomic Decomposition with Statistical Operators* | Tracks energy, mean direction, and variance through reflection, refraction, scattering, and absorption; supports editable textured layers. | Larger material architecture and approximate angular statistics; excessive scope for fixing one opaque GGX interface. | Revisit for rough clearcoat and multilayer materials. [Author's paper and code](https://belcour.github.io/blog/research/publication/2018/05/05/brdf-realtime-layered.html) |
| **Portsmouth, Kutz & Hill, 2025**, *EON: A Practical Energy-Preserving Rough Diffuse BRDF*; preprint 2024, revised 2026 | Analytic compensation for rough Oren–Nayar-style diffuse; rough appearance without its usual energy deficit; supplied evaluation and sampling code. | Solves rough diffuse, not GGX specular or specular/diffuse coupling; would also need a suitable IBL approximation. | Optional later upgrade. Zenith's current Lambert lobe already integrates to its albedo. [Current JCGT paper](https://jcgt.org/published/0014/01/06/paper.pdf) |

OpenPBR is useful as a material-composition reference: it distinguishes mixing material populations from layering interfaces, and permits several microfacet compensation approaches. Adopting its entire material model is not required for this change. [OpenPBR specification](https://academysoftwarefoundation.github.io/OpenPBR/)

## 3. Findings in this renderer

| Area | Current implementation | Consequence |
| --- | --- | --- |
| Specular | [brdf.slang](E:/MyProjects/zenith/content/shaders/brdf.slang:46): GGX and height-correlated Smith visibility. [lighting.slang](E:/MyProjects/zenith/content/shaders/lighting.slang:73): Schlick `F0 = lerp(0.04, base, metallic)`. | A suitable single-scattering foundation. Preserve the exact NDF/visibility pairing when generating energy data. |
| Roughness | [ibl_sampling.slang](E:/MyProjects/zenith/content/shaders/ibl_sampling.slang:3): perceptual roughness floor `0.045`; shaders use `alpha = roughness^2`, and the NDF receives `alpha^2`. | LUT coordinates are perceptual roughness, not alpha. Mixing these conventions invalidates compensation. |
| BRDF LUT | [brdf_lut.slang](E:/MyProjects/zenith/content/shaders/brdf_lut.slang:16): 1,024 Hammersley/NDF samples, storing `(A,B)`. [ibl.rs](E:/MyProjects/zenith/zenith-renderer/src/ibl.rs:90): 128×128 RG32F, generated at initialization. | `A+B` already gives unit-Fresnel directional albedo. No replacement GGX table is needed. |
| Lobe composition | [lighting.slang](E:/MyProjects/zenith/content/shaders/lighting.slang:84): direct diffuse uses `1-F(l,h)`; IBL diffuse uses a roughness-adjusted view Fresnel. | Direct and environment paths do not use a common integrated energy budget. |
| Environment | [lighting.slang](E:/MyProjects/zenith/content/shaders/lighting.slang:92): prefiltered cubemap plus diffuse SH. [sh.slang](E:/MyProjects/zenith/content/shaders/sh.slang:50) evaluates the stored diffuse convolution. | SH already includes Lambert's `1/pi`; a white environment returns approximately one. Do not divide it by pi again. |
| Visibility/output | [lighting.slang](E:/MyProjects/zenith/content/shaders/lighting.slang:85) applies ray-query shadows to direct lighting and AO to diffuse IBL. [world.rs](E:/MyProjects/zenith/zenith-renderer/src/world.rs:837) creates HDR lighting before post-processing. | Validate local BRDF energy with AO/shadows disabled and read back the HDR target before exposure/tone mapping. |
| Existing tests | [IBL tests](E:/MyProjects/zenith/zenith-renderer/src/ibl/tests.rs:102) read back LUT/SH/cubemaps. [Lighting tests](E:/MyProjects/zenith/zenith-renderer/src/world/tests/lighting.rs:158) check material response in output pixels. | Useful infrastructure, but no full BRDF conservation/reciprocity suite. Some assertions explicitly expect rough metals to darken. |

### Numerical check performed for this report

I integrated the current shader equations on the CPU using double precision and 262,144 Hammersley samples per point. These are equation-level results, not rendered-image measurements or GPU performance results.

For a unit-Fresnel metal, uniform unit environment, and `NoV = 1`:

| Perceptual roughness | Single-scattering response `A+B` | Missing fraction |
| --- | ---: | ---: |
| 0.045 | 0.999996 | approximately 0% |
| 0.25 | 0.995690 | 0.43% |
| 0.50 | 0.915814 | 8.42% |
| 0.75 | 0.626888 | 37.31% |
| 1.00 | 0.306855 | 69.31% |

There is an independent analytic check at `alpha = 1`. Zenith's visibility gives:

\[
E(\mu)=1-\mu\ln\frac{1+\mu}{\mu},\qquad
E(1)=1-\ln 2=0.3068528194.
\]

The normal-incidence numerical result differs by about `1.9e-6`. The actual LUT's last texel uses `roughness = NoV = 127.5/128`, so it should not be compared directly with the endpoint: its current 1,024-sample equation gives approximately `0.311976`, versus `0.311534` with 262,144 samples.

For a white dielectric at roughness one, the current IBL equation becomes `0.96 + 0.04*A + B`. The continuum calculation gives **0.972306 at NoV=1**, but **1.024717 at NoV=0.05**. Thus the composition can both lose energy and exceed the white-environment input. Finite LUT resolution changes the exact pixel values, not the need to fix energy sharing.

## 4. Proposed BRDF

All quantities below use linear RGB, fixed surface normal, `mu = saturate(n·direction)`, and Zenith's current roughness mapping. RGB products and divisions are componentwise. The derivation assumes reflectance inputs in `[0,1]` and numerically accurate integrals.

### 4.1 Reuse and extend the integrated data

Define the existing LUT values through the Fresnel decomposition:

\[
S_{ss}(\mu)=F_0 A(\mu)+B(\mu),\qquad E(\mu)=A(\mu)+B(\mu).
\]

Compute two additional roughness-dependent averages:

\[
\bar A=2\int_0^1 A(\mu)\mu\,d\mu,\qquad
\bar B=2\int_0^1 B(\mu)\mu\,d\mu,\qquad
\bar E=\bar A+\bar B.
\]

Keep the existing 128×128 RG32F LUT and add a **1×128 RG32F average LUT**, storing `(Abar,Bbar)` by roughness. Its raw payload is 1 KiB; actual Vulkan allocation overhead is larger. A row-reduction compute pass can derive it from the existing LUT. Integrate its bilinear/clamp-to-edge representation consistently, including the edge intervals, so normalization matches runtime lookup.

### 4.2 Add the corrected Kulla–Conty lobe

\[
\bar F=F_0+(1-F_0)/21,\qquad
C=\frac{\bar F^2\bar E}{1-\bar F(1-\bar E)},
\]

\[
f_{ms}(l,v)=C\frac{[1-E(l)][1-E(v)]}{\pi(1-\bar E)},\qquad
f_s=f_{ss}+f_{ms}.
\]

Use the **squared average Fresnel** in `C`; older versions of the derivation omitted a factor. This changes colored-metal absorption. [Hill's correction](https://blog.selfshadow.com/2018/06/04/multi-faceted-part-2/), [Filament's documented formulation](https://google.github.io/filament/main/filament.html)

For `F0=1`, `C=1`, and integrating the extra lobe returns exactly `1-E(v)`. The total is one. The expression is symmetric in light and view. For bounded `F0`, `Sss<=E` and `0<=C<=1`, so the compensated specular integral remains at most one. These are algebraic properties; finite LUT error still needs measurement.

Its directional and average energy are:

\[
S(v)=F_0 A(v)+B(v)+C[1-E(v)],
\]

\[
\bar S=F_0\bar A+\bar B+C(1-\bar E).
\]

### 4.3 Give diffuse the remaining budget

Keep Zenith's material mapping `rho = (1-metallic)*base`. Couple the diffuse lobe to the **compensated** specular energy:

\[
f_d(l,v)=\frac{\rho}{\pi}
\frac{[1-S(l)][1-S(v)]}{1-\bar S}.
\]

This applies the normalized separable coupling construction to the new specular lobe. It is an energy-budget approximation for opaque materials, not an exact simulation of a dielectric coating over a scattering volume. [Kelemen and Szirmay-Kalos, Section 2.2](https://cg.iit.bme.hu/~szirmay/scook.pdf)

Its integral is `rho*(1-S(v))`; therefore:

\[
R(v)=S(v)+\rho[1-S(v)]\le1.
\]

A white dielectric has `rho=1` and preserves energy. A metal has `rho=0`. Colored absorption remains allowed. Unlike the current half-vector Fresnel diffuse weight, this construction explicitly accounts for the integrated specular lobe.

**Numerical limits:** return zero for compensation when its missing-energy limit vanishes; return zero per diffuse channel when its remaining budget vanishes. Handle the smooth/unit-reflectance `0/0` limits before division. Check LUT validity before applying tiny roundoff clamps; do not mask significant table errors with a final radiance clamp. Derive `S`, `Sbar`, and the evaluated lobes from the same corrected data.

## 5. Environment-light implementation

Let `P(v,r)` be Zenith's current prefiltered specular cubemap sample and `D(n)` its diffuse SH evaluation. A practical integration of the proposed model is:

\[
L_{IBL}\approx P(v,r)S_{ss}(v)
+D(n)\left(C[1-E(v)]+\rho[1-S(v)]\right).
\]

The extra specular scattering uses the broad environment term and retains its specular identity. It is **not multiplied by diffuse base color or `(1-metallic)`**. Use the same `S(v)` for the diffuse budget as for direct lighting.

This is a proposed Zenith approximation: it factors out the incident-angle dependence of the broad lobes. It is exact for a constant environment given accurate tables and convolutions, but can disagree with direct integration for a narrow bright environment feature. The existing single-scattering split sum also remains approximate. Fdez-Agüera motivates using diffuse irradiance for multiple scattering under broad illumination. [IBL paper](https://jcgt.org/published/0008/01/03/paper.pdf)

If this error matters, preconvolve the actual missing-energy kernels per roughness. For this isotropic model, the required incident kernels can be built from `1`, `A(mu)`, and `B(mu)`, then combined with material coefficients at runtime. That adds storage and sky-update work; benchmark the simple version first.

For initial AO integration, apply the existing AO factor to the substrate diffuse term. Keep compensation with the specular terms. Applying diffuse AO to broad specular scattering is a separate occlusion approximation requiring scene comparison. White-furnace validation uses `AO=1` and unobstructed lighting.

### Cheaper alternatives, with Zenith's LUT convention

**Turquin/Filament-style scaling:**

```text
E = A + B
W = 1 + F0 * (1 / E - 1)
direct_specular = W * single_scattering_specular
ibl_specular = prefiltered_radiance * W * (F0*A + B)
```

Filament's documented multiscattering LUT stores different channels: its energy channel corresponds to **Zenith's `A+B`, not Zenith's `B`**. Copying `1/dfg.y` into the current shader would be incorrect. Diffuse still needs a compatible budget. `f(l,v)=W(v)fss(l,v)` is generally unequal to its swapped evaluation. [Filament's LUT convention](https://google.github.io/filament/main/filament.html)

**Fdez-Agüera IBL alternative:**

```text
missing = 1 - (A+B)
Favg = F0 + (1-F0)/21
Sss = F0*A+B
Sms = Sss*Favg*missing / (1-Favg*missing)
Lo = P*Sss + D*(Sms + rho*(1-Sss-Sms))
```

Here `Sms` replaces the chosen IBL compensation; it is not an additional correction on top of Kulla–Conty or Turquin. Use the Fresnel decomposition matching Zenith's LUT rather than introducing another roughness-Fresnel substitution. [Equations 17–18 and dielectric budget](https://jcgt.org/published/0008/01/03/paper.pdf)

## 6. Implementation sequence

### Phase 1 — Establish the numerical contract

1. Add a CPU reference integration for `fss`, its `(A,B)` coefficients, and the proposed composed BRDF. Use independent quadrature/importance sampling and convergence checks, rather than relying only on the shader's 1,024-sample sequence.
2. Cover both fixed-view furnace integration and fixed-light outgoing-power integration. Add reciprocity checks without including the external `NoL` shading factor.
3. Extend the existing IBL readback test to verify `A+B`, interpolation, roughness endpoints, and the analytic `alpha=1` result. Test the actual texel coordinates.
4. Measure LUT errors near grazing incidence. Increase samples or use visible-normal sampling only if needed; a changed sampling PDF must be reflected in the estimator. Sampling improvements alone do not restore multiple scattering.

### Phase 2 — Add average energy data and direct-light evaluation

1. In [ibl.rs](E:/MyProjects/zenith/zenith-renderer/src/ibl.rs:90), allocate the average LUT and add a dependent row-reduction pass after `integrate_brdf`. Add a proposed shader at `E:\MyProjects\zenith\content\shaders\brdf_average.slang`.
2. Export/import the average LUT with `IblResources`; retain its resources through GPU completion. Generate it once per renderer, independently of skybox replacement.
3. Update shader compilation setup in [world.rs](E:/MyProjects/zenith/zenith-renderer/src/world.rs:206), Rust/Slang root layouts in [lighting.rs](E:/MyProjects/zenith/zenith-renderer/src/lighting.rs:87) and [lighting.slang](E:/MyProjects/zenith/content/shaders/lighting.slang:10), and declare the graph's new fragment-read dependency.
4. Add small evaluation helpers in [brdf.slang](E:/MyProjects/zenith/content/shaders/brdf.slang:46). Fetch `(A,B)` for view and light, plus `(Abar,Bbar)` once per pixel. Evaluate `fss+fms+fd`, then multiply the whole direct contribution by light radiance, `NoL`, and shadow visibility.
5. Share intermediate energy terms between the direct and environment paths. This requires no new G-buffer channels, material asset fields, or external dependencies.

### Phase 3 — Apply IBL and finish material composition

1. Replace the current roughness-Fresnel diffuse IBL weight with `rho*(1-S(v))` and add the proposed broad specular term.
2. Keep the existing specular prefilter and diffuse SH initially. Validate constant and structured environments separately.
3. Add development comparison modes for baseline versus the selected method, preferably as shader/debug variants. A permanent per-material method selector is unnecessary.
4. Revisit assertions in [lighting tests](E:/MyProjects/zenith/zenith-renderer/src/world/tests/lighting.rs:158) that require rough-metal darkening. Replace them with energy and material-response checks; highlight broadening should still change directional-light images.

### Phase 4 — Acceptance and profiling

| Check | Proposed acceptance criterion |
| --- | --- |
| White lossless metal and white dielectric | Linear response within 1% of one over the production roughness/view grid, including dedicated grazing bins. This is a target, not a measured pass. |
| General energy | Nonnegative finite response; integrated reflectance no more than `1 + numerical tolerance` per channel for bounded inputs. Check both integration directions. |
| Reciprocity | Relative/absolute mixed tolerance around `1e-5` for CPU evaluation; account separately for GPU precision. Include widely separated light/view angles. |
| Colored and mixed materials | Sweep gray, saturated RGB, representative conductor colors, and metallic values `0, 0.5, 1`; allow physical absorption. |
| Numerical boundaries | Minimum roughness, `NoV/NoL` near zero, degenerate half vectors, `F0=0/1`, table borders, and smooth missing-energy limits produce no NaNs or negative lobes. |
| Direct/IBL consistency | Constant environment agrees with hemisphere integration of the direct BRDF within LUT/convolution tolerance. Narrow HDR features are evaluated as approximation error, not expected exact matches. |
| GPU readback | Read the HDR lighting target before post-processing; add test-only transfer/readback support. Current byte/BMP output is insufficient for energy validation. |
| Scene regressions | Use the sphere grid and Cerberus in [pbr_preview.rs](E:/MyProjects/zenith/zenith-sandbox/examples/pbr_preview.rs:42), with separate direct, IBL, and combined captures at fixed exposure; compare shadows and AO independently. |
| Cost | Profile release shaders at fixed resolution/scene over repeated warmed frames. Record lighting-pass GPU time separately from LUT startup time and full-frame time. |

Run the existing relevant GPU checks once implementation exists:

```powershell
cargo test -p zenith-renderer sh_convolution_specular_prefilter_and_lut -- --ignored --nocapture
cargo test -p zenith-renderer directional_and_skylight_follow_material_parameters -- --ignored --nocapture
cargo run -p zenith-sandbox --example pbr_preview --release
cargo run -p zenith-sandbox --example pbr_preview --release -- cerberus
```

These require the repository's supported Vulkan GPU, Slang, and validation configuration. Then run the normal workspace checks for the implemented change. No GPU acceptance tests or renderer builds were run for this research-only report.

### Expected engineering cost

- Current LUT: 128 KiB raw payload. New average LUT: 1 KiB plus allocation/descriptor overhead.
- Direct + IBL shading: reuse the current view-LUT fetch; add one light-LUT fetch and one average-LUT fetch, plus arithmetic. The average is reusable across future lights; each additional light needs its directional lookup.
- Startup: one small reduction after the existing integration. No new environment prefilter in the initial version.
- Planning estimate: **4–7 focused engineering days**, including numerical reference tests, Slang/Rust integration, GPU readback, and comparisons. This assumes the current renderer and GPU test environment work. Treat it as an estimate, not a benchmark or delivery commitment.

## 7. Later BxDF extensions

- **Rough transmission:** introduce a reflection/transmission BSDF with IOR and total-internal-reflection handling. Its missing energy depends on both hemispheres; the current conductor-style `F=1` reflection table is insufficient. Turquin describes joint normalization with IOR-dependent data for entering/exiting interfaces. [Technical report, Section 3.2](https://blog.selfshadow.com/publications/turquin/ms_comp_final.pdf)
- **Anisotropy:** replace the isotropic `(NoV, roughness)` energy parameterization with one that includes anisotropic roughness and azimuth, or validate a fitted approximation.
- **Clearcoat/sheen:** allocate energy to the upper layer and attenuate the underlying material consistently. Do not simply add independently normalized lobes. Belcour and OpenPBR are the appropriate next design references.
- **Path tracing:** add a consistent `eval/sample/pdf` contract. A changed BxDF need not have an exactly matching importance distribution to be unbiased, but every evaluated contribution needs sampling support and the correct PDF. For the added lobes, start with a GGX/cosine mixture and its complete mixture PDF; use the stochastic Smith model for comparison.
- **Normal mapping:** the conservation proof is in the chosen local normal frame. Geometric-normal clipping and shading-normal transport corrections need separate tests; a LUT cannot repair arbitrary normal-map transport errors.

The recommended first milestone is a **validated opaque material model with one shared energy budget for direct and environment lighting**. Expand to transmission, anisotropy, and layered materials after that foundation is measured.
