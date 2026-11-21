<script lang="ts">
  // 바탕에 흩날리는 꽃잎(결정 0062). 연하고 반투명한 꽃잎이 위에서 아래로 천천히 떨어진다.
  // 장식일 뿐이다: 보조 기술에 숨기고, 누를 수 없고, 카드·글 뒤에만 그려진다(틀의 빈 자리에서 보인다).
  // 움직임 줄이기를 켠 기기, 강제 색 모드, 인쇄에서는 그리지 않는다.
  // `fill`: 놓인 상자를 가득 채운다(Solo). 주지 않으면 화면 높이만큼의 층이 스크롤을 따라 붙어 있다(Shell).
  let { fill = false }: { fill?: boolean } = $props();
  const petals = Array.from({ length: 18 }, (_, i) => i);
</script>

<div class="petals" class:fill aria-hidden="true">
  <div class="sky">
    {#each petals as i (i)}<span class="p" class:alt={i % 2 === 1}><i></i></span>{/each}
  </div>
</div>

<style>
  /* 높이를 차지하지 않는 붙박이 층: 자리는 흐름에서 0이고, 안의 하늘이 화면 높이만큼 펼쳐진다. */
  .petals {
    position: sticky;
    top: 0;
    height: 0;
    pointer-events: none;
  }
  .sky {
    position: absolute;
    inset: 0 0 auto 0;
    height: 100dvh;
    overflow: clip;
  }
  .petals.fill {
    position: absolute;
    inset: 0;
    z-index: -1;
    height: auto;
    overflow: clip;
  }
  .fill .sky {
    height: 100%;
  }
  /* 꽃잎 한 장: 바깥(.p)은 떨어지며 옆으로 흐르고, 안(i)은 좌우로 흔들리며 돈다. */
  .p {
    position: absolute;
    top: 0;
    left: var(--x);
    width: calc(var(--size) * 1.2);
    height: calc(var(--size) * 1.38);
    animation: fall var(--dur) linear var(--delay) infinite;
    will-change: transform;
  }
  .p i {
    display: block;
    width: 100%;
    height: 100%;
    background: var(--drift);
    opacity: calc(var(--drift-alpha) * var(--a));
    -webkit-mask: url('./motif/petal.svg') center / contain no-repeat;
    mask: url('./motif/petal.svg') center / contain no-repeat;
    animation: sway-a var(--sway) ease-in-out var(--delay) infinite alternate;
  }
  .alt i {
    animation-name: sway-b;
  }
  @keyframes fall {
    from { transform: translate3d(0, -6vh, 0); }
    to { transform: translate3d(var(--dx), 106vh, 0); }
  }
  @keyframes sway-a {
    from { transform: translateX(-1.1rem) rotate(-35deg) rotateY(0deg); }
    to { transform: translateX(1.1rem) rotate(150deg) rotateY(60deg); }
  }
  @keyframes sway-b {
    from { transform: translateX(0.9rem) rotate(40deg) rotateX(55deg); }
    to { transform: translateX(-0.9rem) rotate(-130deg) rotateX(0deg); }
  }
  /* 꽃잎마다 자리·크기·빠르기·진하기가 다르다(인라인 스타일을 쓰지 않으려고 여기 적는다. CSP). */
  .p:nth-child(1) { --x: 78.9%; --size: 1.1rem; --dur: 17.8s; --delay: -10.0s; --dx: 15.4vw; --sway: 5.1s; --a: 0.55; }
  .p:nth-child(2) { --x: 14.2%; --size: 1.1rem; --dur: 17.6s; --delay: -3.9s; --dx: 10.7vw; --sway: 4.6s; --a: 0.55; }
  .p:nth-child(3) { --x: 53.1%; --size: 1.25rem; --dur: 27.6s; --delay: -5.0s; --dx: 11.0vw; --sway: 4.4s; --a: 0.55; }
  .p:nth-child(4) { --x: 91.9%; --size: 1.25rem; --dur: 19.7s; --delay: -13.4s; --dx: 9.1vw; --sway: 5.1s; --a: 1.0; }
  .p:nth-child(5) { --x: 41.1%; --size: 0.9rem; --dur: 27.3s; --delay: -19.1s; --dx: 6.9vw; --sway: 4.9s; --a: 0.85; }
  .p:nth-child(6) { --x: 37.0%; --size: 1.0rem; --dur: 24.9s; --delay: -1.8s; --dx: 10.1vw; --sway: 4.3s; --a: 1.0; }
  .p:nth-child(7) { --x: 63.6%; --size: 1.4rem; --dur: 18.0s; --delay: -10.1s; --dx: -13.5vw; --sway: 4.3s; --a: 0.85; }
  .p:nth-child(8) { --x: 19.8%; --size: 1.25rem; --dur: 27.4s; --delay: -1.9s; --dx: 5.1vw; --sway: 5.5s; --a: 0.55; }
  .p:nth-child(9) { --x: 73.2%; --size: 1.4rem; --dur: 21.0s; --delay: -12.2s; --dx: 12.2vw; --sway: 5.6s; --a: 0.85; }
  .p:nth-child(10) { --x: 28.6%; --size: 1.1rem; --dur: 21.6s; --delay: -13.2s; --dx: 9.9vw; --sway: 4.1s; --a: 0.7; }
  .p:nth-child(11) { --x: 85.7%; --size: 1.6rem; --dur: 23.5s; --delay: -3.9s; --dx: 8.8vw; --sway: 3.7s; --a: 1.0; }
  .p:nth-child(12) { --x: 48.7%; --size: 1.0rem; --dur: 26.2s; --delay: -25.8s; --dx: 12.2vw; --sway: 4.0s; --a: 0.55; }
  .p:nth-child(13) { --x: 7.0%; --size: 0.9rem; --dur: 25.6s; --delay: -0.3s; --dx: 14.0vw; --sway: 4.1s; --a: 0.7; }
  .p:nth-child(14) { --x: 2.5%; --size: 1.0rem; --dur: 24.9s; --delay: -7.9s; --dx: -5.5vw; --sway: 6.3s; --a: 0.55; }
  .p:nth-child(15) { --x: 97.0%; --size: 1.6rem; --dur: 27.1s; --delay: -23.7s; --dx: 13.6vw; --sway: 4.5s; --a: 0.55; }
  .p:nth-child(16) { --x: 69.4%; --size: 1.1rem; --dur: 17.8s; --delay: -1.2s; --dx: 6.5vw; --sway: 4.3s; --a: 0.55; }
  .p:nth-child(17) { --x: 23.4%; --size: 1.25rem; --dur: 19.0s; --delay: -1.9s; --dx: 8.4vw; --sway: 6.1s; --a: 1.0; }
  .p:nth-child(18) { --x: 56.9%; --size: 1.0rem; --dur: 29.4s; --delay: -17.7s; --dx: 9.7vw; --sway: 4.8s; --a: 1.0; }
  @media (prefers-reduced-motion: reduce) {
    .petals {
      display: none;
    }
  }
  @media (forced-colors: active) {
    .petals {
      display: none;
    }
  }
  @media print {
    .petals {
      display: none;
    }
  }
</style>
